//! Dedicated live cloud transport for the Libraxis Voice Lab WebSocket.
//!
//! This module owns normal live capture: a `set` message, bounded base64 PCM
//! `chunk` messages, client `flush` commits, and a bounded `end`/drain. While
//! the handshake is in progress, PCM sits in a time-bounded pre-connect buffer
//! sized for `connect_timeout` at up to 192 kHz and drains in order when the
//! socket opens. After that, the small live queue is unchanged: a slow server
//! still surfaces overflow. The receive adapter converts Voice Lab events into
//! Codescribe's normalized vocabulary and stamps each untimed final with the
//! capture-clock span the client committed.
//! Whole-file multipart upload lives outside this session and is reserved for
//! explicit retranscribe actions. This module does not own recorder wiring,
//! consent, or provider selection.
//!
//! Provider ordering is evidence, not authority. Transcript revisions are
//! compared only inside their utterance, duplicates and stale revisions are
//! removed, and [`LiveCloudAsrSession`] assigns a fresh Codescribe-owned
//! stream-global sequence to every event it emits.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Highest native capture rate the pre-connect buffer is sized for.
const MAX_NATIVE_CAPTURE_HZ: u64 = 192_000;

/// Wall-clock cadence of the periodic flush used until the first explicit commit.
const PERIODIC_FLUSH_INTERVAL: Duration = Duration::from_millis(2_500);

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use super::consent::CloudEgressAuthorization;
use super::events::{
    AsrErrorKind, AsrSessionEvent, AudioRange, ErrorEvent, SessionId, TranscriptEvent, UsageEvent,
};
use super::provider::{AsrSessionProvider, RefinerMode, SessionInput};

/// Normalized bounds for one live cloud session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudSessionLimits {
    /// Largest PCM callback accepted as one gateway frame.
    pub max_frame_samples: usize,
    /// Maximum wire items inspected by one non-blocking [`drain`](AsrSessionProvider::drain).
    pub max_events_per_drain: usize,
    /// Maximum trailing wire items accepted while synchronously closing.
    pub max_close_events: usize,
    /// Bounded audio/end command queue feeding the socket worker.
    pub outbound_queue_capacity: usize,
    /// Bounded normalized event queue returning from the socket worker.
    pub inbound_queue_capacity: usize,
    /// Maximum remembered gateway event ids used for replay suppression.
    pub remembered_event_ids: usize,
    /// Upper bound for the WebSocket handshake.
    pub connect_timeout: Duration,
    /// Upper bound for one socket send.
    pub send_timeout: Duration,
    /// Upper bound for the end signal and trailing receive drain.
    pub close_timeout: Duration,
}

impl Default for CloudSessionLimits {
    fn default() -> Self {
        Self {
            // 200 ms at the expected 16 kHz input rate.
            max_frame_samples: 3_200,
            max_events_per_drain: 64,
            max_close_events: 128,
            outbound_queue_capacity: 8,
            inbound_queue_capacity: 128,
            remembered_event_ids: 4_096,
            connect_timeout: Duration::from_secs(10),
            send_timeout: Duration::from_secs(5),
            close_timeout: Duration::from_secs(2),
        }
    }
}

impl CloudSessionLimits {
    fn validate(&self) -> Result<(), AsrErrorKind> {
        if self.max_frame_samples == 0
            || self.max_events_per_drain == 0
            || self.max_close_events == 0
            || self.outbound_queue_capacity == 0
            || self.inbound_queue_capacity == 0
            || self.remembered_event_ids == 0
            || self.connect_timeout.is_zero()
            || self.send_timeout.is_zero()
            || self.close_timeout.is_zero()
        {
            return Err(AsrErrorKind::Protocol);
        }
        Ok(())
    }
}

/// Live endpoint and its endpoint-owned authentication credential.
///
/// Its `Debug` representation is deliberately content-free. Endpoints can carry
/// signed query parameters and bearer values are credentials; neither belongs
/// in logs, panic output, or telemetry.
pub struct GatewayConnection {
    endpoint: String,
    credential: String,
    auth_mode: crate::stt::tail_provider::SttAuthMode,
}

impl GatewayConnection {
    /// Validate a normalized gateway WebSocket connection.
    ///
    /// Remote plaintext and URL user-info are refused. A signed query string is
    /// allowed but remains redacted by the type's `Debug` implementation.
    pub fn new(
        endpoint: impl Into<String>,
        credential: impl Into<String>,
    ) -> Result<Self, AsrErrorKind> {
        let endpoint = endpoint.into();
        let credential = credential.into();
        let parsed = reqwest::Url::parse(&endpoint).map_err(|_| AsrErrorKind::Protocol)?;
        let host = parsed
            .host_str()
            .map(|value| value.trim_matches(['[', ']']))
            .ok_or(AsrErrorKind::Protocol)?;
        let encrypted = parsed.scheme() == "wss";
        let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
        let auth_mode = crate::stt::tail_provider::stt_auth_mode(&endpoint);
        if !(encrypted || parsed.scheme() == "ws" && loopback)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || (auth_mode != crate::stt::tail_provider::SttAuthMode::Unauthenticated
                && credential.trim().is_empty()
                && crate::llm::speech::vendor_for_endpoint(&endpoint).is_none())
        {
            return Err(AsrErrorKind::Protocol);
        }

        if auth_mode != crate::stt::tail_provider::SttAuthMode::Unauthenticated {
            HeaderValue::from_str(credential.trim()).map_err(|_| AsrErrorKind::Protocol)?;
        }
        Ok(Self {
            endpoint,
            credential,
            auth_mode,
        })
    }
}

impl fmt::Debug for GatewayConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayConnection")
            .field("endpoint", &"[REDACTED]")
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

/// Provider-neutral session configuration sent exactly once after connect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GatewaySessionConfig {
    #[serde(rename = "type")]
    message_type: &'static str,
    protocol_version: u16,
    session_id: String,
    locale: Option<String>,
    /// Codescribe domain token. The gateway must not classify audio to pick one.
    vocabulary: &'static str,
    audio: GatewayAudioConfig,
}

impl GatewaySessionConfig {
    fn from_input(input: &SessionInput) -> Self {
        Self {
            message_type: "session.start",
            protocol_version: 1,
            session_id: input.session_id.as_str().to_string(),
            locale: input.locale.clone(),
            vocabulary: crate::stt::request_vocabulary::CODESCRIBE_STT_VOCABULARY,
            audio: GatewayAudioConfig {
                encoding: "pcm_s16le",
                sample_rate_hz: input.sample_rate,
                channels: 1,
                frame_header: "sequence_u64_be",
            },
        }
    }

    /// Opaque session id echoed by every gateway event.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Audio sample rate sent to the gateway.
    pub fn sample_rate_hz(&self) -> u32 {
        self.audio.sample_rate_hz
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct GatewayAudioConfig {
    encoding: &'static str,
    sample_rate_hz: u32,
    channels: u8,
    frame_header: &'static str,
}

/// One bounded mono PCM16-LE frame, prefixed on the wire by its local send id.
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayPcmFrame {
    sequence_id: u64,
    pcm_s16le: Vec<u8>,
}

impl GatewayPcmFrame {
    /// Codescribe-owned monotonic audio frame number.
    pub fn sequence_id(&self) -> u64 {
        self.sequence_id
    }

    /// PCM payload length, excluding the eight-byte sequence header.
    pub fn payload_len(&self) -> usize {
        self.pcm_s16le.len()
    }

    #[cfg(test)]
    fn into_wire_bytes(self) -> Vec<u8> {
        let mut wire = Vec::with_capacity(8 + self.pcm_s16le.len());
        wire.extend_from_slice(&self.sequence_id.to_be_bytes());
        wire.extend_from_slice(&self.pcm_s16le);
        wire
    }
}

/// Stateful adapter from the proven Voice Lab wire into Codescribe's strict
/// normalized event vocabulary.
struct VoiceLabReceiveState {
    session_id: String,
    next_event_id: u64,
    utterance_id: u64,
    revision: u64,
    xai: bool,
    xai_done: bool,
}

impl VoiceLabReceiveState {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            next_event_id: 1,
            utterance_id: 1,
            revision: 0,
            xai: false,
            xai_done: false,
        }
    }

    fn event_id(&mut self) -> Result<String, AsrErrorKind> {
        let id = self.next_event_id;
        self.next_event_id = self
            .next_event_id
            .checked_add(1)
            .ok_or(AsrErrorKind::Protocol)?;
        Ok(format!("voice-lab-{id}"))
    }

    // xAI emits chunk finals and complete utterance finals. Only speech_final
    // seals our utterance; transcript.done is session-wide accounting, never
    // another transcript occurrence (its text repeats the whole session).
    fn adapt_xai(&mut self, text: &str) -> Result<Option<GatewayEvent>, AsrErrorKind> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|_| AsrErrorKind::Protocol)?;
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("transcript.created") => Ok(None),
            Some("transcript.done") => {
                self.xai_done = true;
                Ok(Some(GatewayEvent::SessionEnded {
                    session_id: self.session_id.clone(),
                }))
            }
            Some("error") => Err(AsrErrorKind::Protocol),
            Some("transcript.partial") => {
                let text = value
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(AsrErrorKind::Protocol)?
                    .to_string();
                let is_final = value
                    .get("is_final")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                    && value
                        .get("speech_final")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                let start = value.get("start").and_then(serde_json::Value::as_f64);
                let duration = value.get("duration").and_then(serde_json::Value::as_f64);
                let milliseconds = |seconds: f64| {
                    (seconds.is_finite() && seconds >= 0.0)
                        .then_some((seconds * 1000.0).round() as u64)
                };
                let start_ms = start.and_then(milliseconds);
                let end_ms = start
                    .zip(duration)
                    .and_then(|(start, duration)| milliseconds(start + duration));
                self.revision = self.revision.checked_add(1).ok_or(AsrErrorKind::Protocol)?;
                let event_id = self.event_id()?;
                let event = if is_final {
                    GatewayEvent::Final {
                        event_id,
                        session_id: self.session_id.clone(),
                        utterance_id: self.utterance_id,
                        revision: self.revision,
                        text,
                        start_ms,
                        end_ms,
                        commit_id: None,
                        item_id: None,
                    }
                } else {
                    GatewayEvent::Partial {
                        event_id,
                        session_id: self.session_id.clone(),
                        utterance_id: self.utterance_id,
                        revision: self.revision,
                        text,
                        start_ms,
                        end_ms,
                    }
                };
                if is_final {
                    self.utterance_id = self
                        .utterance_id
                        .checked_add(1)
                        .ok_or(AsrErrorKind::Protocol)?;
                    self.revision = 0;
                }
                Ok(Some(event))
            }
            _ => Err(AsrErrorKind::Protocol),
        }
    }

    fn adapt(&mut self, text: &str) -> Result<Option<GatewayEvent>, AsrErrorKind> {
        if self.xai {
            return self.adapt_xai(text);
        }
        if let Ok(event) = serde_json::from_str::<GatewayEvent>(text) {
            return Ok(Some(event));
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|_| AsrErrorKind::Protocol)?;
        let message_type = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or(AsrErrorKind::Protocol)?;
        match message_type {
            // Voice Lab `stt-ws-v1` opens with `hello` and then control/VAD
            // frames. Those are not transcript events; treating them as a
            // protocol fault used to drop Layer 1 at take start.
            "ack" | "ready" | "hello" | "vad.sample" | "speech.start" | "speech.end" => Ok(None),
            "transcript.partial" | "transcript" => {
                let Some(text) = voice_lab_text(&value) else {
                    return Ok(None);
                };
                self.revision = self.revision.checked_add(1).ok_or(AsrErrorKind::Protocol)?;
                let event_id = self.event_id()?;
                Ok(Some(GatewayEvent::Partial {
                    event_id,
                    session_id: self.session_id.clone(),
                    utterance_id: self.utterance_id,
                    revision: self.revision,
                    text,
                    start_ms: None,
                    end_ms: None,
                }))
            }
            "transcript.final" => {
                // An empty final is still a commit boundary. Dropping it would
                // leave the client's oldest span attached to a later final.
                let text = voice_lab_final_text(&value);
                self.revision = self.revision.checked_add(1).ok_or(AsrErrorKind::Protocol)?;
                let event_id = self.event_id()?;
                let event = GatewayEvent::Final {
                    event_id,
                    session_id: self.session_id.clone(),
                    utterance_id: self.utterance_id,
                    revision: self.revision,
                    text,
                    start_ms: None,
                    end_ms: None,
                    commit_id: json_string(&value, "commit_id"),
                    item_id: json_string(&value, "item_id"),
                };
                self.utterance_id = self
                    .utterance_id
                    .checked_add(1)
                    .ok_or(AsrErrorKind::Protocol)?;
                self.revision = 0;
                Ok(Some(event))
            }
            "error" => {
                let code = match value.get("code").and_then(serde_json::Value::as_str) {
                    Some("auth" | "unauthorized" | "forbidden") => GatewayErrorCode::Auth,
                    Some("quota" | "payment_required") => GatewayErrorCode::Quota,
                    Some("rate_limited") => GatewayErrorCode::RateLimited,
                    Some("timeout") => GatewayErrorCode::Timeout,
                    Some("backpressure") => GatewayErrorCode::Backpressure,
                    _ => GatewayErrorCode::Protocol,
                };
                let event_id = self.event_id()?;
                Ok(Some(GatewayEvent::Error {
                    event_id,
                    session_id: self.session_id.clone(),
                    utterance_id: self.utterance_id,
                    code,
                }))
            }
            "end" | "session.ended" | "stream.closed" => Ok(Some(GatewayEvent::SessionEnded {
                session_id: self.session_id.clone(),
            })),
            _ => Ok(None),
        }
    }
}

fn voice_lab_text(value: &serde_json::Value) -> Option<String> {
    value
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Final text, including the empty string. Partials still use [`voice_lab_text`].
fn voice_lab_final_text(value: &serde_json::Value) -> String {
    value
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Voice Lab live start frame. The engine's frozen inbound types are
/// `set` / `chunk` / `flush` / `end` — `config` is rejected as unknown.
fn voice_lab_set_message(config: &GatewaySessionConfig) -> String {
    serde_json::json!({
        "type": "set",
        "language": config.locale.as_deref().unwrap_or("pl"),
        "sample_rate": config.sample_rate_hz(),
        "encoding": "pcm16",
        "vocabulary": config.vocabulary,
        // The client owns segmentation. Server VAD must not cut the audio.
        "vad": false,
    })
    .to_string()
}

fn voice_lab_flush_message() -> String {
    serde_json::json!({"type": "flush"}).to_string()
}

fn voice_lab_end_message() -> String {
    serde_json::json!({"type": "end"}).to_string()
}

/// Samples of audio the pre-connect buffer holds for one handshake window.
fn preconnect_sample_capacity(connect_timeout: Duration) -> u64 {
    let samples = u128::from(MAX_NATIVE_CAPTURE_HZ).saturating_mul(connect_timeout.as_nanos())
        / 1_000_000_000;
    u64::try_from(samples).unwrap_or(u64::MAX).max(1)
}

impl fmt::Debug for GatewayPcmFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayPcmFrame")
            .field("sequence_id", &self.sequence_id)
            .field("payload_bytes", &self.pcm_s16le.len())
            .finish()
    }
}

/// Stable normalized gateway error vocabulary. No vendor message crosses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayErrorCode {
    /// Connection credentials are missing, expired, or rejected.
    Auth,
    /// A transient request-rate limit.
    RateLimited,
    /// Billing or account quota is exhausted.
    Quota,
    /// The requested locale or session capability is unsupported.
    Unsupported,
    /// Gateway/session protocol mismatch.
    Protocol,
    /// Gateway-side buffering could not keep up.
    Backpressure,
    /// A normalized gateway deadline elapsed.
    Timeout,
    /// The gateway cancelled the session.
    Cancelled,
}

impl GatewayErrorCode {
    fn as_asr_kind(self) -> AsrErrorKind {
        match self {
            Self::Auth => AsrErrorKind::Auth,
            Self::RateLimited => AsrErrorKind::RateLimited,
            Self::Quota => AsrErrorKind::Quota,
            Self::Unsupported => AsrErrorKind::Unsupported,
            Self::Protocol => AsrErrorKind::Protocol,
            Self::Backpressure => AsrErrorKind::Overflow,
            Self::Timeout => AsrErrorKind::Transport,
            Self::Cancelled => AsrErrorKind::Cancelled,
        }
    }
}

/// Provider-neutral receive vocabulary spoken by the Libraxis gateway.
///
/// `revision` is scoped only to its utterance. It is used to discard stale
/// provider frames and is never exposed as the Codescribe event sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum GatewayEvent {
    /// Volatile transcript hypothesis.
    #[serde(rename = "transcript.partial")]
    Partial {
        event_id: String,
        session_id: String,
        utterance_id: u64,
        revision: u64,
        text: String,
        #[serde(default)]
        start_ms: Option<u64>,
        #[serde(default)]
        end_ms: Option<u64>,
    },
    /// Sealing transcript hypothesis.
    #[serde(rename = "transcript.final")]
    Final {
        event_id: String,
        session_id: String,
        utterance_id: u64,
        revision: u64,
        text: String,
        #[serde(default)]
        start_ms: Option<u64>,
        #[serde(default)]
        end_ms: Option<u64>,
        /// Client commit id echoed by a vendor, when the final carries one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit_id: Option<String>,
        /// OpenAI Realtime item id, when the final carries one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
    },
    /// Typed error without provider prose.
    #[serde(rename = "session.error")]
    Error {
        event_id: String,
        session_id: String,
        #[serde(default)]
        utterance_id: u64,
        code: GatewayErrorCode,
    },
    /// Content-free accounting.
    #[serde(rename = "session.usage")]
    Usage {
        event_id: String,
        session_id: String,
        audio_ms: u64,
        #[serde(default)]
        billable_units: Option<u64>,
    },
    /// Explicit acknowledgement that trailing events are complete.
    #[serde(rename = "session.ended")]
    SessionEnded { session_id: String },
}

impl GatewayEvent {
    fn session_id(&self) -> &str {
        match self {
            Self::Partial { session_id, .. }
            | Self::Final { session_id, .. }
            | Self::Error { session_id, .. }
            | Self::Usage { session_id, .. }
            | Self::SessionEnded { session_id } => session_id,
        }
    }

    fn event_id(&self) -> Option<&str> {
        match self {
            Self::Partial { event_id, .. }
            | Self::Final { event_id, .. }
            | Self::Error { event_id, .. }
            | Self::Usage { event_id, .. } => Some(event_id),
            Self::SessionEnded { .. } => None,
        }
    }
}

/// Non-blocking result of polling an injected gateway transport.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayTransportPoll {
    /// No receive item is ready now.
    Pending,
    /// One normalized gateway event is ready.
    Event(GatewayEvent),
    /// The transport failed with a content-free typed reason.
    Fault(AsrErrorKind),
    /// The transport ended and no more events can arrive.
    Closed,
}

/// Injectable boundary between the session adapter and a WebSocket actor.
pub trait CloudGatewayTransport: Send {
    /// Start one normalized session.
    fn start(&mut self, config: GatewaySessionConfig) -> Result<(), AsrErrorKind>;
    /// Queue one bounded PCM frame without waiting for socket I/O.
    fn try_send_pcm(&mut self, frame: GatewayPcmFrame) -> Result<(), AsrErrorKind>;
    /// Queue one vendor commit. Libraxis sends `flush`.
    ///
    /// The default succeeds without I/O so existing transports keep compiling.
    fn commit_flush(&mut self, commit_id: &str) -> Result<(), AsrErrorKind> {
        let _ = commit_id;
        Ok(())
    }
    /// Poll one receive item without blocking.
    fn poll(&mut self) -> GatewayTransportPoll;
    /// Queue the normalized end signal.
    fn begin_end(&mut self) -> Result<(), AsrErrorKind>;
    /// Cancel any remaining work after a bounded drain expires.
    fn abort(&mut self);
}

#[derive(Debug)]
enum GatewayCommand {
    Pcm(GatewayPcmFrame),
    Flush,
    End,
    Abort,
}

/// One outbound item held until the socket handshake finishes.
#[derive(Debug)]
enum BufferedOutbound {
    Pcm(GatewayPcmFrame),
    Flush,
    End,
}

/// PCM accepted during the handshake, bounded by capture time.
struct PreconnectBuffer {
    commands: VecDeque<BufferedOutbound>,
    samples: u64,
    capacity_samples: u64,
}

impl PreconnectBuffer {
    fn covering(connect_timeout: Duration) -> Self {
        Self {
            commands: VecDeque::new(),
            samples: 0,
            capacity_samples: preconnect_sample_capacity(connect_timeout),
        }
    }

    /// Accept one frame. Exceeding the handshake window is a disconnect.
    fn push_pcm(&mut self, frame: GatewayPcmFrame) -> Result<(), AsrErrorKind> {
        let samples = u64::try_from(frame.pcm_s16le.len() / 2).unwrap_or(u64::MAX);
        if self.samples.saturating_add(samples) > self.capacity_samples {
            return Err(AsrErrorKind::Transport);
        }
        self.samples = self.samples.saturating_add(samples);
        self.commands.push_back(BufferedOutbound::Pcm(frame));
        Ok(())
    }

    fn push_flush(&mut self) {
        self.commands.push_back(BufferedOutbound::Flush);
    }

    fn push_end(&mut self) {
        self.commands.push_back(BufferedOutbound::End);
    }

    fn take_commands(&mut self) -> VecDeque<BufferedOutbound> {
        self.samples = 0;
        std::mem::take(&mut self.commands)
    }
}

struct SharedOutbound {
    buffer: PreconnectBuffer,
    /// Handshake finished; further commands use the live queue.
    live: bool,
    aborted: bool,
    failed: Option<AsrErrorKind>,
}

fn lock_outbound(shared: &Mutex<SharedOutbound>) -> std::sync::MutexGuard<'_, SharedOutbound> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn mark_outbound_failed(shared: &Mutex<SharedOutbound>, kind: AsrErrorKind) {
    let mut guard = lock_outbound(shared);
    if guard.failed.is_none() {
        guard.failed = Some(kind);
    }
}

#[derive(Debug)]
enum WorkerSignal {
    Event(GatewayEvent),
    Fault(AsrErrorKind),
    Closed,
}

/// Real bounded WebSocket actor for the Voice Lab wire contract.
///
/// The socket and credential live on a dedicated current-thread Tokio runtime.
/// The synchronous provider side only performs bounded `try_send`/`try_recv`
/// channel operations; it never performs network I/O on the audio callback.
pub struct GatewayWebSocketTransport {
    connection: Option<GatewayConnection>,
    limits: CloudSessionLimits,
    shared: Arc<Mutex<SharedOutbound>>,
    command_tx: Option<mpsc::Sender<GatewayCommand>>,
    event_rx: Option<mpsc::Receiver<WorkerSignal>>,
    worker: Option<JoinHandle<()>>,
    started: bool,
    ending: bool,
}

impl GatewayWebSocketTransport {
    /// Build a dormant gateway transport. Network I/O starts at session open.
    pub fn new(
        connection: GatewayConnection,
        limits: CloudSessionLimits,
    ) -> Result<Self, AsrErrorKind> {
        limits.validate()?;
        let shared = Arc::new(Mutex::new(SharedOutbound {
            buffer: PreconnectBuffer::covering(limits.connect_timeout),
            live: false,
            aborted: false,
            failed: None,
        }));
        Ok(Self {
            connection: Some(connection),
            limits,
            shared,
            command_tx: None,
            event_rx: None,
            worker: None,
            started: false,
            ending: false,
        })
    }

    fn lock_shared(&self) -> std::sync::MutexGuard<'_, SharedOutbound> {
        lock_outbound(&self.shared)
    }

    fn live_send(&self, command: GatewayCommand) -> Result<(), AsrErrorKind> {
        if let Some(kind) = self.lock_shared().failed {
            return Err(kind);
        }
        let sender = self.command_tx.as_ref().ok_or(AsrErrorKind::Transport)?;
        sender.try_send(command).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => AsrErrorKind::Overflow,
            mpsc::error::TrySendError::Closed(_) => {
                self.lock_shared().failed.unwrap_or(AsrErrorKind::Transport)
            }
        })
    }
}

impl fmt::Debug for GatewayWebSocketTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayWebSocketTransport")
            .field("connection", &self.connection)
            .field("started", &self.started)
            .field("ending", &self.ending)
            .finish_non_exhaustive()
    }
}

impl CloudGatewayTransport for GatewayWebSocketTransport {
    fn start(&mut self, config: GatewaySessionConfig) -> Result<(), AsrErrorKind> {
        if self.started {
            return Err(AsrErrorKind::Protocol);
        }

        let (command_tx, command_rx) = mpsc::channel(self.limits.outbound_queue_capacity);
        let (event_tx, event_rx) = mpsc::channel(self.limits.inbound_queue_capacity);
        // Move the credential into the socket worker. The synchronous
        // provider retains no spare credential copy after session start.
        let connection = self.connection.take().ok_or(AsrErrorKind::Protocol)?;
        let limits = self.limits;
        let shared = Arc::clone(&self.shared);
        let worker = std::thread::Builder::new()
            .name("codescribe-live-cloud-asr".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    mark_outbound_failed(&shared, AsrErrorKind::Transport);
                    let _ = event_tx.blocking_send(WorkerSignal::Fault(AsrErrorKind::Transport));
                    let _ = event_tx.blocking_send(WorkerSignal::Closed);
                    return;
                };
                runtime.block_on(gateway_worker(
                    connection, config, limits, command_rx, event_tx, shared,
                ));
            })
            .map_err(|_| AsrErrorKind::Transport)?;

        self.command_tx = Some(command_tx);
        self.event_rx = Some(event_rx);
        self.worker = Some(worker);
        self.started = true;
        Ok(())
    }

    fn try_send_pcm(&mut self, frame: GatewayPcmFrame) -> Result<(), AsrErrorKind> {
        if !self.started || self.ending {
            return Err(AsrErrorKind::Protocol);
        }
        {
            let mut shared = self.lock_shared();
            if let Some(kind) = shared.failed {
                return Err(kind);
            }
            if !shared.live {
                return shared.buffer.push_pcm(frame);
            }
        }
        self.live_send(GatewayCommand::Pcm(frame))
    }

    fn commit_flush(&mut self, _commit_id: &str) -> Result<(), AsrErrorKind> {
        if !self.started || self.ending {
            return Err(AsrErrorKind::Protocol);
        }
        {
            let mut shared = self.lock_shared();
            if let Some(kind) = shared.failed {
                return Err(kind);
            }
            if !shared.live {
                shared.buffer.push_flush();
                return Ok(());
            }
        }
        self.live_send(GatewayCommand::Flush)
    }

    fn poll(&mut self) -> GatewayTransportPoll {
        let Some(receiver) = self.event_rx.as_mut() else {
            return GatewayTransportPoll::Pending;
        };
        match receiver.try_recv() {
            Ok(WorkerSignal::Event(event)) => GatewayTransportPoll::Event(event),
            Ok(WorkerSignal::Fault(kind)) => GatewayTransportPoll::Fault(kind),
            Ok(WorkerSignal::Closed) => GatewayTransportPoll::Closed,
            Err(mpsc::error::TryRecvError::Empty) => GatewayTransportPoll::Pending,
            Err(mpsc::error::TryRecvError::Disconnected) => GatewayTransportPoll::Closed,
        }
    }

    fn begin_end(&mut self) -> Result<(), AsrErrorKind> {
        if !self.started || self.ending {
            return Err(AsrErrorKind::Protocol);
        }
        let buffered_end = {
            let mut shared = self.lock_shared();
            if let Some(kind) = shared.failed {
                return Err(kind);
            }
            if !shared.live {
                shared.buffer.push_end();
                true
            } else {
                false
            }
        };
        if buffered_end {
            self.ending = true;
            return Ok(());
        }
        self.live_send(GatewayCommand::End)?;
        self.ending = true;
        Ok(())
    }

    fn abort(&mut self) {
        self.lock_shared().aborted = true;
        if let Some(sender) = self.command_tx.take() {
            let _ = sender.try_send(GatewayCommand::Abort);
        }
        self.ending = true;
    }
}

impl Drop for GatewayWebSocketTransport {
    fn drop(&mut self) {
        self.abort();
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
}

async fn gateway_worker(
    connection: GatewayConnection,
    config: GatewaySessionConfig,
    limits: CloudSessionLimits,
    command_rx: mpsc::Receiver<GatewayCommand>,
    event_tx: mpsc::Sender<WorkerSignal>,
    shared: Arc<Mutex<SharedOutbound>>,
) {
    let result =
        run_gateway_socket(connection, config, limits, command_rx, &event_tx, &shared).await;
    if let Err(kind) = result {
        mark_outbound_failed(&shared, kind);
        let _ = event_tx.send(WorkerSignal::Fault(kind)).await;
    }
    let _ = event_tx.send(WorkerSignal::Closed).await;
}

async fn run_gateway_socket(
    connection: GatewayConnection,
    config: GatewaySessionConfig,
    limits: CloudSessionLimits,
    mut command_rx: mpsc::Receiver<GatewayCommand>,
    event_tx: &mpsc::Sender<WorkerSignal>,
    shared: &Mutex<SharedOutbound>,
) -> Result<(), AsrErrorKind> {
    let vendor = crate::llm::speech::vendor_for_endpoint(&connection.endpoint);
    let xai = vendor == Some(crate::llm::provider::ProviderKind::XaiResponses);
    if vendor == Some(crate::llm::provider::ProviderKind::OpenAiResponses) {
        return Err(AsrErrorKind::Unsupported); // OpenAI live STT is a separate protocol.
    }
    let auth = if let Some(vendor) = vendor {
        Some(
            crate::llm::speech::resolve_vendor_auth(vendor, Some(&connection.credential))
                .await
                .map_err(|_| AsrErrorKind::Auth)?,
        )
    } else {
        None
    };
    let credential = auth
        .as_ref()
        .map_or(connection.credential.as_str(), |auth| auth.bearer.as_str());
    let endpoint = if xai {
        xai_live_endpoint(&connection.endpoint, &config)?
    } else {
        connection.endpoint.clone()
    };
    let mut request = endpoint
        .as_str()
        .into_client_request()
        .map_err(|_| AsrErrorKind::Protocol)?;
    match connection.auth_mode {
        crate::stt::tail_provider::SttAuthMode::Unauthenticated => {}
        crate::stt::tail_provider::SttAuthMode::Bearer => {
            let authorization = HeaderValue::from_str(&format!("Bearer {}", credential.trim()))
                .map_err(|_| AsrErrorKind::Protocol)?;
            request.headers_mut().insert(AUTHORIZATION, authorization);
        }
        crate::stt::tail_provider::SttAuthMode::ApiKey => {
            let value = HeaderValue::from_str(connection.credential.trim())
                .map_err(|_| AsrErrorKind::Protocol)?;
            request
                .headers_mut()
                .insert(HeaderName::from_static("x-api-key"), value);
        }
    }

    let connected = match timeout(limits.connect_timeout, connect_async(request)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return Err(classify_socket_error(&error)),
        Err(_elapsed) => return Err(AsrErrorKind::Transport),
    };
    let (mut socket, _) = connected;

    // Publish the socket before any further send. Audio offered during the
    // handshake is already in `backlog`; later frames use the live queue.
    let backlog = {
        let mut guard = lock_outbound(shared);
        if guard.aborted {
            None
        } else {
            guard.live = true;
            Some(guard.buffer.take_commands())
        }
    };
    let Some(backlog) = backlog else {
        let _ = socket.close(None).await;
        return Ok(());
    };

    let mut receive_state = VoiceLabReceiveState::new(config.session_id.clone());
    receive_state.xai = xai;

    // Proven Voice Lab wire: credentials stay in the WebSocket handshake,
    // never in the JSON body. The engine start type is `set`, not `config`.
    if xai {
        timeout(
            limits.connect_timeout,
            wait_xai_created(&mut socket, limits.send_timeout),
        )
        .await
        .map_err(|_| AsrErrorKind::Transport)??;
    } else {
        send_socket_message(
            &mut socket,
            Message::Text(voice_lab_set_message(&config).into()),
            limits.send_timeout,
        )
        .await?;
    }

    for command in backlog {
        let command = match command {
            BufferedOutbound::Pcm(frame) => GatewayCommand::Pcm(frame),
            BufferedOutbound::Flush => GatewayCommand::Flush,
            BufferedOutbound::End => GatewayCommand::End,
        };
        if apply_gateway_command(
            command,
            xai,
            &config,
            &mut socket,
            limits,
            event_tx,
            &mut receive_state,
        )
        .await?
        {
            return Ok(());
        }
    }

    loop {
        tokio::select! {
            command = command_rx.recv() => {
                match command {
                    Some(command) => {
                        if apply_gateway_command(
                            command,
                            xai,
                            &config,
                            &mut socket,
                            limits,
                            event_tx,
                            &mut receive_state,
                        )
                        .await?
                        {
                            return Ok(());
                        }
                    }
                    None => {
                        let _ = socket.close(None).await;
                        return Ok(());
                    }
                }
            }
            incoming = socket.next() => {
                if forward_gateway_message(
                    incoming,
                    &mut socket,
                    limits.send_timeout,
                    event_tx,
                    &mut receive_state,
                ).await? {
                    return Err(AsrErrorKind::Transport);
                }
            }
        }
    }
}

/// Send one command. `true` means the socket task should stop.
async fn apply_gateway_command(
    command: GatewayCommand,
    xai: bool,
    config: &GatewaySessionConfig,
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    limits: CloudSessionLimits,
    event_tx: &mpsc::Sender<WorkerSignal>,
    receive_state: &mut VoiceLabReceiveState,
) -> Result<bool, AsrErrorKind> {
    match command {
        GatewayCommand::Pcm(frame) => {
            if xai {
                send_socket_message(
                    socket,
                    Message::Binary(frame.pcm_s16le.into()),
                    limits.send_timeout,
                )
                .await?;
                return Ok(false);
            }
            let chunk = serde_json::json!({
                "type": "chunk",
                "audio_base64": BASE64.encode(&frame.pcm_s16le),
                "sample_rate": config.audio.sample_rate_hz,
                "encoding": "pcm16",
            })
            .to_string();
            send_socket_message(socket, Message::Text(chunk.into()), limits.send_timeout).await?;
            Ok(false)
        }
        GatewayCommand::Flush => {
            if !xai {
                send_socket_message(
                    socket,
                    Message::Text(voice_lab_flush_message().into()),
                    limits.send_timeout,
                )
                .await?;
            }
            Ok(false)
        }
        GatewayCommand::End => {
            if xai {
                send_socket_message(
                    socket,
                    Message::Text(r#"{"type":"audio.done"}"#.into()),
                    limits.send_timeout,
                )
                .await?;
            } else {
                // `end` is itself the final commit. A flush ahead of it would
                // emit an extra final with nothing left to anchor.
                send_socket_message(
                    socket,
                    Message::Text(voice_lab_end_message().into()),
                    limits.send_timeout,
                )
                .await?;
            }
            drain_gateway_tail(socket, limits, event_tx, receive_state).await?;
            Ok(true)
        }
        GatewayCommand::Abort => {
            let _ = socket.close(None).await;
            Ok(true)
        }
    }
}

fn xai_live_endpoint(
    endpoint: &str,
    config: &GatewaySessionConfig,
) -> Result<String, AsrErrorKind> {
    let mut url = reqwest::Url::parse(endpoint).map_err(|_| AsrErrorKind::Protocol)?;
    if url.path() != "/v1/stt" {
        return Err(AsrErrorKind::Unsupported);
    }
    // The capture format belongs to the recording session, never a URL override.
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("encoding", "pcm")
            .append_pair("sample_rate", &config.audio.sample_rate_hz.to_string())
            .append_pair("interim_results", "true");
        if let Some(language) = config.locale.as_deref() {
            query.append_pair("language", language);
        }
    }
    Ok(url.into())
}

async fn wait_xai_created(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    send_timeout: Duration,
) -> Result<(), AsrErrorKind> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let value: serde_json::Value =
                    serde_json::from_str(&text).map_err(|_| AsrErrorKind::Protocol)?;
                return if value.get("type").and_then(serde_json::Value::as_str)
                    == Some("transcript.created")
                {
                    Ok(())
                } else {
                    Err(AsrErrorKind::Protocol)
                };
            }
            Some(Ok(Message::Ping(payload))) => {
                send_socket_message(socket, Message::Pong(payload), send_timeout).await?
            }
            Some(Ok(Message::Pong(_))) => {}
            _ => return Err(AsrErrorKind::Transport),
        }
    }
}

async fn drain_gateway_tail(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    limits: CloudSessionLimits,
    event_tx: &mpsc::Sender<WorkerSignal>,
    receive_state: &mut VoiceLabReceiveState,
) -> Result<(), AsrErrorKind> {
    let deadline = tokio::time::Instant::now() + limits.close_timeout;
    loop {
        let incoming = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| AsrErrorKind::Transport)?;
        if forward_gateway_message(
            incoming,
            socket,
            limits.send_timeout,
            event_tx,
            receive_state,
        )
        .await?
        {
            return if receive_state.xai && !receive_state.xai_done {
                Err(AsrErrorKind::Transport)
            } else {
                Ok(())
            };
        }
    }
}

async fn forward_gateway_message(
    incoming: Option<Result<Message, WebSocketError>>,
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    send_timeout: Duration,
    event_tx: &mpsc::Sender<WorkerSignal>,
    receive_state: &mut VoiceLabReceiveState,
) -> Result<bool, AsrErrorKind> {
    match incoming {
        Some(Ok(Message::Text(text))) => {
            let Some(event) = receive_state.adapt(text.as_ref())? else {
                return Ok(false);
            };
            let ended = matches!(event, GatewayEvent::SessionEnded { .. });
            event_tx
                .send(WorkerSignal::Event(event))
                .await
                .map_err(|_| AsrErrorKind::Cancelled)?;
            Ok(ended)
        }
        Some(Ok(Message::Ping(payload))) => {
            send_socket_message(socket, Message::Pong(payload), send_timeout).await?;
            Ok(false)
        }
        Some(Ok(Message::Pong(_))) => Ok(false),
        Some(Ok(Message::Close(_))) | None => Ok(true),
        Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => Err(AsrErrorKind::Protocol),
        Some(Err(error)) => Err(classify_socket_error(&error)),
    }
}

async fn send_socket_message(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    message: Message,
    send_timeout: Duration,
) -> Result<(), AsrErrorKind> {
    timeout(send_timeout, socket.send(message))
        .await
        .map_err(|_| AsrErrorKind::Transport)?
        .map_err(|error| classify_socket_error(&error))
}

fn classify_socket_error(error: &WebSocketError) -> AsrErrorKind {
    if let WebSocketError::Http(response) = error {
        return match response.status().as_u16() {
            401 | 403 => AsrErrorKind::Auth,
            402 => AsrErrorKind::Quota,
            429 => AsrErrorKind::RateLimited,
            400..=499 => AsrErrorKind::Protocol,
            _ => AsrErrorKind::Transport,
        };
    }
    AsrErrorKind::Transport
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Idle,
    Open,
    Ending,
    Closed,
    Failed,
}

/// Content-free counters safe to place in diagnostics and telemetry.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CloudSessionTelemetry {
    /// PCM frames accepted by the bounded transport queue.
    pub frames_queued: u64,
    /// Samples accepted by the bounded transport queue.
    pub samples_queued: u64,
    /// Normalized events emitted to the caller.
    pub events_emitted: u64,
    /// Exact gateway replays suppressed by event id.
    pub duplicate_events: u64,
    /// Old per-utterance revisions or post-final updates suppressed.
    pub stale_events: u64,
    /// Bounded send attempts refused by backpressure.
    pub backpressure_events: u64,
    /// Transport-level faults normalized into typed events.
    pub transport_faults: u64,
}

#[derive(Debug)]
struct SeenEventIds {
    capacity: usize,
    order: VecDeque<String>,
    values: HashSet<String>,
}

impl SeenEventIds {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            values: HashSet::with_capacity(capacity),
        }
    }

    fn insert(&mut self, event_id: &str) -> bool {
        if self.values.contains(event_id) {
            return false;
        }
        if self.order.len() == self.capacity
            && let Some(expired) = self.order.pop_front()
        {
            self.values.remove(&expired);
        }
        let owned = event_id.to_string();
        self.order.push_back(owned.clone());
        self.values.insert(owned);
        true
    }
}

/// Live cloud implementation of [`AsrSessionProvider`].
/// One client commit waiting for the final that seals it.
struct PendingCommit {
    commit_id: String,
    range: AudioRange,
}

/// How a recorded commit is asked of the vendor.
enum CommitWire {
    /// Libraxis `flush`. The periodic path uses this until the first explicit commit.
    Flush,
    /// `end` is the last commit and must not be preceded by another flush.
    End,
}

pub struct LiveCloudAsrSession<T: CloudGatewayTransport> {
    _authorization: CloudEgressAuthorization,
    transport: T,
    limits: CloudSessionLimits,
    state: SessionState,
    session_id: Option<SessionId>,
    /// Native rate from [`SessionInput::sample_rate`] at open. See [`offered_capture_rate_hz`].
    capture_rate_hz: u32,
    /// Samples accepted by [`AsrSessionProvider::push_audio`], on the capture clock.
    capture_samples_pushed: u64,
    /// Exclusive end of the last recorded commit, in capture samples.
    last_commit_sample: u64,
    next_commit_number: u64,
    pending_commits: VecDeque<PendingCommit>,
    /// Once set, the 2.5 s flush stops. The caller owns segmentation.
    explicit_commit: bool,
    last_periodic_flush: Instant,
    next_audio_sequence: u64,
    next_event_sequence: u64,
    utterance_revisions: HashMap<u64, u64>,
    sealed_utterances: HashSet<u64>,
    seen_event_ids: SeenEventIds,
    ready: VecDeque<AsrSessionEvent>,
    telemetry: CloudSessionTelemetry,
    fault_seen: bool,
}

/// Native capture rate the recorder offered this session, in Hz.
///
/// Commit ranges count `push_audio` samples at this rate. The value is
/// [`SessionInput::sample_rate`] and is not rewritten to the 16 kHz wire rate.
fn offered_capture_rate_hz(input: &SessionInput) -> u32 {
    input.sample_rate
}

impl<T: CloudGatewayTransport> LiveCloudAsrSession<T> {
    /// Build an explicitly authorized live session over an injected normalized transport.
    pub fn new(
        transport: T,
        limits: CloudSessionLimits,
        authorization: CloudEgressAuthorization,
    ) -> Result<Self, AsrErrorKind> {
        limits.validate()?;
        Ok(Self {
            _authorization: authorization,
            transport,
            limits,
            state: SessionState::Idle,
            session_id: None,
            capture_rate_hz: 0,
            capture_samples_pushed: 0,
            last_commit_sample: 0,
            next_commit_number: 1,
            pending_commits: VecDeque::new(),
            explicit_commit: false,
            last_periodic_flush: Instant::now(),
            next_audio_sequence: 1,
            next_event_sequence: 1,
            utterance_revisions: HashMap::new(),
            sealed_utterances: HashSet::new(),
            seen_event_ids: SeenEventIds::new(limits.remembered_event_ids),
            ready: VecDeque::new(),
            telemetry: CloudSessionTelemetry::default(),
            fault_seen: false,
        })
    }

    /// Borrow the injected transport, primarily for deterministic verification.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Content-free session counters.
    pub fn telemetry(&self) -> CloudSessionTelemetry {
        self.telemetry
    }

    fn session_id(&self) -> Result<SessionId, AsrErrorKind> {
        self.session_id.clone().ok_or(AsrErrorKind::Protocol)
    }

    fn take_event_sequence(&mut self) -> u64 {
        let sequence_id = self.next_event_sequence;
        self.next_event_sequence = self.next_event_sequence.saturating_add(1);
        sequence_id
    }

    fn queue_local_error(&mut self, utterance_id: u64, kind: AsrErrorKind) {
        if let Ok(session_id) = self.session_id() {
            let sequence_id = self.take_event_sequence();
            self.ready.push_back(AsrSessionEvent::Error(ErrorEvent {
                session_id,
                utterance_id,
                sequence_id,
                kind,
            }));
        }
    }

    /// Record `[last_commit_sample, commit_sample)` and ask the transport to commit it.
    fn enqueue_commit(&mut self, commit_sample: u64, wire: CommitWire) -> Result<(), AsrErrorKind> {
        if commit_sample < self.last_commit_sample || commit_sample > self.capture_samples_pushed {
            return Err(AsrErrorKind::Protocol);
        }
        if commit_sample == self.last_commit_sample {
            return match wire {
                CommitWire::End => self.transport.begin_end(),
                CommitWire::Flush => Ok(()),
            };
        }
        let start_sample = self.last_commit_sample;
        let range =
            AudioRange::from_capture_samples(start_sample, commit_sample, self.capture_rate_hz)
                .ok_or(AsrErrorKind::Protocol)?;
        let commit_id = format!("cs-commit-{}", self.next_commit_number);
        // Record before the wire send so a final polled immediately after
        // this call still finds its span.
        self.pending_commits.push_back(PendingCommit {
            commit_id: commit_id.clone(),
            range,
        });
        self.last_commit_sample = commit_sample;
        self.next_commit_number = self.next_commit_number.saturating_add(1);
        let sent = match wire {
            CommitWire::Flush => self.transport.commit_flush(&commit_id),
            CommitWire::End => self.transport.begin_end(),
        };
        if let Err(kind) = sent {
            self.pending_commits.pop_back();
            self.last_commit_sample = start_sample;
            self.next_commit_number = self.next_commit_number.saturating_sub(1);
            return Err(kind);
        }
        Ok(())
    }

    /// Vendor id first, then the oldest commit. Unknown ids do not skip the queue.
    fn take_pending_commit(
        &mut self,
        commit_id: Option<&str>,
        item_id: Option<&str>,
    ) -> Option<PendingCommit> {
        let echoed = commit_id
            .filter(|value| !value.is_empty())
            .or_else(|| item_id.filter(|value| !value.is_empty()));
        if let Some(id) = echoed
            && let Some(index) = self
                .pending_commits
                .iter()
                .position(|pending| pending.commit_id == id)
        {
            return self.pending_commits.remove(index);
        }
        self.pending_commits.pop_front()
    }

    /// Send one flush for audio accepted since the previous commit.
    ///
    /// Stops after the first explicit [`AsrSessionProvider::commit`].
    fn maybe_periodic_flush(&mut self) -> Result<(), AsrErrorKind> {
        if self.explicit_commit || self.state != SessionState::Open {
            return Ok(());
        }
        if self.last_periodic_flush.elapsed() < PERIODIC_FLUSH_INTERVAL {
            return Ok(());
        }
        if self.capture_samples_pushed == self.last_commit_sample {
            return Ok(());
        }
        self.enqueue_commit(self.capture_samples_pushed, CommitWire::Flush)?;
        self.last_periodic_flush = Instant::now();
        Ok(())
    }

    fn normalize(&mut self, event: GatewayEvent) {
        let expected_session = match self.session_id.as_ref() {
            Some(value) => value.as_str(),
            None => return,
        };
        if event.session_id() != expected_session {
            self.queue_local_error(0, AsrErrorKind::Protocol);
            return;
        }
        if matches!(event, GatewayEvent::SessionEnded { .. }) {
            self.state = SessionState::Closed;
            return;
        }

        let Some(event_id) = event.event_id() else {
            self.queue_local_error(0, AsrErrorKind::Protocol);
            return;
        };
        if event_id.trim().is_empty() {
            self.queue_local_error(0, AsrErrorKind::Protocol);
            return;
        }
        if !self.seen_event_ids.insert(event_id) {
            self.telemetry.duplicate_events += 1;
            return;
        }

        let normalized = match event {
            GatewayEvent::Partial {
                utterance_id,
                revision,
                text,
                start_ms,
                end_ms,
                ..
            } => self.normalize_transcript(
                false,
                utterance_id,
                revision,
                text,
                start_ms,
                end_ms,
                None,
                None,
            ),
            GatewayEvent::Final {
                utterance_id,
                revision,
                text,
                start_ms,
                end_ms,
                commit_id,
                item_id,
                ..
            } => self.normalize_transcript(
                true,
                utterance_id,
                revision,
                text,
                start_ms,
                end_ms,
                commit_id,
                item_id,
            ),
            GatewayEvent::Error {
                utterance_id, code, ..
            } => {
                let session_id = self.session_id();
                let sequence_id = self.take_event_sequence();
                session_id.map(|session_id| {
                    Some(AsrSessionEvent::Error(ErrorEvent {
                        session_id,
                        utterance_id,
                        sequence_id,
                        kind: code.as_asr_kind(),
                    }))
                })
            }
            GatewayEvent::Usage {
                audio_ms,
                billable_units,
                ..
            } => {
                let session_id = self.session_id();
                let sequence_id = self.take_event_sequence();
                session_id.map(|session_id| {
                    Some(AsrSessionEvent::Usage(UsageEvent {
                        session_id,
                        utterance_id: 0,
                        sequence_id,
                        audio_secs: duration_millis_to_secs(audio_ms),
                        billable_units,
                    }))
                })
            }
            GatewayEvent::SessionEnded { .. } => return,
        };

        match normalized {
            Ok(Some(event)) => self.ready.push_back(event),
            Ok(None) => {}
            Err(kind) => self.queue_local_error(0, kind),
        }
    }

    // CL-W1b replaces this argument list with one wire-final value and drops the allow.
    #[allow(clippy::too_many_arguments)]
    fn normalize_transcript(
        &mut self,
        is_final: bool,
        utterance_id: u64,
        revision: u64,
        text: String,
        start_ms: Option<u64>,
        end_ms: Option<u64>,
        commit_id: Option<String>,
        item_id: Option<String>,
    ) -> Result<Option<AsrSessionEvent>, AsrErrorKind> {
        if self.sealed_utterances.contains(&utterance_id)
            || self
                .utterance_revisions
                .get(&utterance_id)
                .is_some_and(|previous| revision <= *previous)
        {
            self.telemetry.stale_events += 1;
            return Ok(None);
        }
        let untimed = start_ms.is_none() && end_ms.is_none();
        let range = if is_final && untimed {
            // Libraxis finals carry `duration_ms: null`. The span is the one
            // this client committed, or `AsrErrorKind::Protocol` — never a guess.
            let Some(pending) = self.take_pending_commit(commit_id.as_deref(), item_id.as_deref())
            else {
                return Err(AsrErrorKind::Protocol);
            };
            Some(pending.range)
        } else if text.trim().is_empty() {
            return Err(AsrErrorKind::Protocol);
        } else {
            match (start_ms, end_ms) {
                (None, None) => None,
                (Some(start), Some(end)) => Some(
                    AudioRange::new(duration_millis_to_secs(start), duration_millis_to_secs(end))
                        .ok_or(AsrErrorKind::Protocol)?,
                ),
                _ => return Err(AsrErrorKind::Protocol),
            }
        };
        self.utterance_revisions.insert(utterance_id, revision);
        if is_final {
            self.sealed_utterances.insert(utterance_id);
        }
        let session_id = self.session_id()?;
        let sequence_id = self.take_event_sequence();
        let transcript = TranscriptEvent {
            session_id,
            utterance_id,
            sequence_id,
            text,
            range,
        };
        Ok(Some(if is_final {
            AsrSessionEvent::Final(transcript)
        } else {
            AsrSessionEvent::Partial(transcript)
        }))
    }

    fn poll_transport_once(&mut self) -> bool {
        match self.transport.poll() {
            GatewayTransportPoll::Pending => false,
            GatewayTransportPoll::Event(event) => {
                self.normalize(event);
                true
            }
            GatewayTransportPoll::Fault(kind) => {
                self.telemetry.transport_faults += 1;
                self.fault_seen = true;
                self.queue_local_error(0, kind);
                self.state = SessionState::Failed;
                true
            }
            GatewayTransportPoll::Closed => {
                if !matches!(self.state, SessionState::Ending | SessionState::Closed)
                    && !self.fault_seen
                {
                    self.telemetry.transport_faults += 1;
                    self.queue_local_error(0, AsrErrorKind::Transport);
                }
                self.state = SessionState::Closed;
                true
            }
        }
    }
}

impl<T: CloudGatewayTransport> AsrSessionProvider for LiveCloudAsrSession<T> {
    fn mode(&self) -> RefinerMode {
        RefinerMode::CloudSession
    }

    fn open(&mut self, input: &SessionInput) -> Result<(), AsrErrorKind> {
        if self.state != SessionState::Idle || input.sample_rate == 0 {
            return Err(AsrErrorKind::Protocol);
        }
        self.session_id = Some(input.session_id.clone());
        self.capture_rate_hz = offered_capture_rate_hz(input);
        self.last_periodic_flush = Instant::now();
        if let Err(kind) = self
            .transport
            .start(GatewaySessionConfig::from_input(input))
        {
            self.state = SessionState::Failed;
            return Err(kind);
        }
        self.state = SessionState::Open;
        Ok(())
    }

    fn push_audio(&mut self, samples: &[f32]) -> Result<(), AsrErrorKind> {
        if self.state != SessionState::Open || samples.is_empty() {
            return Err(AsrErrorKind::Protocol);
        }
        self.maybe_periodic_flush()?;
        if samples.len() > self.limits.max_frame_samples {
            self.telemetry.backpressure_events += 1;
            return Err(AsrErrorKind::Overflow);
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(AsrErrorKind::Protocol);
        }
        if self.next_audio_sequence == u64::MAX {
            return Err(AsrErrorKind::Protocol);
        }

        let frame = GatewayPcmFrame {
            sequence_id: self.next_audio_sequence,
            pcm_s16le: samples_to_pcm_s16le(samples),
        };
        match self.transport.try_send_pcm(frame) {
            Ok(()) => {
                self.next_audio_sequence += 1;
                self.capture_samples_pushed = self
                    .capture_samples_pushed
                    .saturating_add(samples.len() as u64);
                self.telemetry.frames_queued += 1;
                self.telemetry.samples_queued += samples.len() as u64;
                Ok(())
            }
            Err(kind) => {
                if kind == AsrErrorKind::Overflow {
                    self.telemetry.backpressure_events += 1;
                }
                Err(kind)
            }
        }
    }

    fn drain(&mut self) -> Vec<AsrSessionEvent> {
        let _ = self.maybe_periodic_flush();
        for _ in 0..self.limits.max_events_per_drain {
            if !self.poll_transport_once() {
                break;
            }
        }
        let drained: Vec<_> = self.ready.drain(..).collect();
        self.telemetry.events_emitted += drained.len() as u64;
        drained
    }

    fn close(&mut self) -> Result<(), AsrErrorKind> {
        if self.state != SessionState::Open {
            return Err(AsrErrorKind::Protocol);
        }
        // `end` commits whatever capture audio is still uncommitted.
        if let Err(kind) = self.enqueue_commit(self.capture_samples_pushed, CommitWire::End) {
            self.transport.abort();
            self.state = SessionState::Failed;
            return Err(kind);
        }
        self.state = SessionState::Ending;

        let deadline = Instant::now() + self.limits.close_timeout;
        let mut close_events = 0usize;
        while Instant::now() < deadline {
            let progressed = self.poll_transport_once();
            if self.state == SessionState::Closed {
                return Ok(());
            }
            if self.state == SessionState::Failed {
                self.transport.abort();
                return Err(AsrErrorKind::Transport);
            }
            if progressed {
                close_events += 1;
                if close_events >= self.limits.max_close_events {
                    self.transport.abort();
                    self.telemetry.transport_faults += 1;
                    self.queue_local_error(0, AsrErrorKind::Overflow);
                    self.state = SessionState::Failed;
                    return Err(AsrErrorKind::Overflow);
                }
            } else {
                // `close` is the one bounded blocking operation in the provider
                // lifecycle. Avoid a hot spin while the socket actor waits for
                // its final/usage/session.ended tail.
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        self.transport.abort();
        self.telemetry.transport_faults += 1;
        self.queue_local_error(0, AsrErrorKind::Transport);
        self.state = SessionState::Failed;
        Err(AsrErrorKind::Transport)
    }

    fn commit(&mut self, commit_sample: u64) -> Result<(), AsrErrorKind> {
        if self.state != SessionState::Open {
            return Err(AsrErrorKind::Protocol);
        }
        self.explicit_commit = true;
        self.enqueue_commit(commit_sample, CommitWire::Flush)
    }
}

fn duration_millis_to_secs(milliseconds: u64) -> f32 {
    Duration::from_millis(milliseconds).as_secs_f32()
}

fn samples_to_pcm_s16le(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let scaled = if sample >= 0.0 {
            (sample.clamp(0.0, 1.0) * f32::from(i16::MAX)).round() as i16
        } else {
            (sample.clamp(-1.0, 0.0) * 32_768.0).round() as i16
        };
        bytes.extend_from_slice(&scaled.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr_session::consent::authorize_cloud_egress;
    use crate::config::cloud_asr::{AudioEgressConsent, ConsentSource};

    #[derive(Debug, Default)]
    struct FakeGatewayTransport {
        started: Vec<GatewaySessionConfig>,
        frames: Vec<GatewayPcmFrame>,
        script: VecDeque<GatewayTransportPoll>,
        send_capacity: Option<usize>,
        ending: bool,
        aborted: bool,
    }

    impl FakeGatewayTransport {
        fn scripted(script: impl IntoIterator<Item = GatewayTransportPoll>) -> Self {
            Self {
                script: script.into_iter().collect(),
                ..Self::default()
            }
        }

        fn with_send_capacity(capacity: usize) -> Self {
            Self {
                send_capacity: Some(capacity),
                ..Self::default()
            }
        }
    }

    impl CloudGatewayTransport for FakeGatewayTransport {
        fn start(&mut self, config: GatewaySessionConfig) -> Result<(), AsrErrorKind> {
            if !self.started.is_empty() {
                return Err(AsrErrorKind::Protocol);
            }
            self.started.push(config);
            Ok(())
        }

        fn try_send_pcm(&mut self, frame: GatewayPcmFrame) -> Result<(), AsrErrorKind> {
            if self
                .send_capacity
                .is_some_and(|capacity| self.frames.len() >= capacity)
            {
                return Err(AsrErrorKind::Overflow);
            }
            self.frames.push(frame);
            Ok(())
        }

        fn poll(&mut self) -> GatewayTransportPoll {
            self.script
                .pop_front()
                .unwrap_or(GatewayTransportPoll::Pending)
        }

        fn begin_end(&mut self) -> Result<(), AsrErrorKind> {
            self.ending = true;
            Ok(())
        }

        fn abort(&mut self) {
            self.aborted = true;
        }
    }

    fn session_id() -> SessionId {
        SessionId::new("gateway-session-1").expect("valid test session")
    }

    fn input() -> SessionInput {
        SessionInput {
            session_id: session_id(),
            locale: Some("pl-PL".to_string()),
            sample_rate: 16_000,
        }
    }

    fn limits() -> CloudSessionLimits {
        CloudSessionLimits {
            max_frame_samples: 4,
            max_events_per_drain: 32,
            max_close_events: 16,
            outbound_queue_capacity: 2,
            inbound_queue_capacity: 16,
            remembered_event_ids: 32,
            connect_timeout: Duration::from_millis(20),
            send_timeout: Duration::from_millis(20),
            close_timeout: Duration::from_millis(20),
        }
    }

    fn authorization() -> CloudEgressAuthorization {
        authorize_cloud_egress(&AudioEgressConsent::Granted(
            ConsentSource::ExplicitSettings,
        ))
        .expect("explicit test consent")
    }

    fn partial(event_id: &str, utterance_id: u64, revision: u64, text: &str) -> GatewayEvent {
        GatewayEvent::Partial {
            event_id: event_id.to_string(),
            session_id: session_id().to_string(),
            utterance_id,
            revision,
            text: text.to_string(),
            start_ms: None,
            end_ms: None,
        }
    }

    fn final_event(event_id: &str, utterance_id: u64, revision: u64, text: &str) -> GatewayEvent {
        GatewayEvent::Final {
            event_id: event_id.to_string(),
            session_id: session_id().to_string(),
            utterance_id,
            revision,
            text: text.to_string(),
            start_ms: None,
            end_ms: None,
            commit_id: None,
            item_id: None,
        }
    }

    fn final_with_item(
        event_id: &str,
        utterance_id: u64,
        text: &str,
        item_id: &str,
    ) -> GatewayEvent {
        let mut event = final_event(event_id, utterance_id, 1, text);
        if let GatewayEvent::Final { item_id: slot, .. } = &mut event {
            *slot = Some(item_id.to_string());
        }
        event
    }

    #[test]
    fn xai_receive_seals_only_complete_utterances_and_does_not_replay_done() {
        let mut state = VoiceLabReceiveState::new("xai-test".into());
        state.xai = true;
        assert!(
            state
                .adapt(r#"{"type":"transcript.created"}"#)
                .unwrap()
                .is_none()
        );
        let chunk = state.adapt(r#"{"type":"transcript.partial","text":"one","is_final":true,"speech_final":false,"start":0,"duration":0.5}"#).unwrap().unwrap();
        assert!(matches!(
            chunk,
            GatewayEvent::Partial {
                utterance_id: 1,
                ..
            }
        ));
        let utterance = state.adapt(r#"{"type":"transcript.partial","text":"one one","is_final":true,"speech_final":true,"start":0,"duration":1.25}"#).unwrap().unwrap();
        assert!(
            matches!(utterance, GatewayEvent::Final { utterance_id: 1, start_ms: Some(0), end_ms: Some(1250), text, .. } if text == "one one")
        );
        let next = state.adapt(r#"{"type":"transcript.partial","text":"one one","is_final":true,"speech_final":true,"start":2,"duration":1}"#).unwrap().unwrap();
        assert!(matches!(
            next,
            GatewayEvent::Final {
                utterance_id: 2,
                ..
            }
        ));
        assert!(matches!(
            state
                .adapt(r#"{"type":"transcript.done","text":"one one one one","duration":3}"#)
                .unwrap(),
            Some(GatewayEvent::SessionEnded { .. })
        ));
    }

    #[test]
    fn xai_connection_defers_oauth_resolution_and_rejects_non_stt_path() {
        assert!(GatewayConnection::new("wss://api.x.ai/v1/stt", "").is_ok());
        let config = GatewaySessionConfig {
            message_type: "session.start",
            protocol_version: 1,
            session_id: "test".into(),
            locale: Some("pl".into()),
            vocabulary: "programming",
            audio: GatewayAudioConfig {
                encoding: "pcm_s16le",
                sample_rate_hz: 16000,
                channels: 1,
                frame_header: "sequence_u64_be",
            },
        };
        let endpoint =
            xai_live_endpoint("wss://api.x.ai/v1/stt?sample_rate=8000", &config).unwrap();
        assert!(endpoint.contains("sample_rate=16000"));
        assert!(endpoint.contains("encoding=pcm"));
        assert!(endpoint.contains("language=pl"));
        assert!(xai_live_endpoint("wss://api.x.ai/v1/realtime", &config).is_err());
    }

    #[test]
    fn normalized_start_and_bounded_pcm_frames_are_sent() {
        let mut session =
            LiveCloudAsrSession::new(FakeGatewayTransport::default(), limits(), authorization())
                .expect("valid limits");
        session.open(&input()).expect("open");
        session
            .push_audio(&[-1.0, -0.5, 0.5, 1.0])
            .expect("bounded frame");

        let transport = session.transport();
        assert_eq!(transport.started.len(), 1);
        assert_eq!(transport.started[0].session_id(), "gateway-session-1");
        assert_eq!(transport.started[0].sample_rate_hz(), 16_000);
        let start_json = serde_json::to_value(&transport.started[0]).expect("serialize start");
        assert_eq!(start_json["type"], "session.start");
        assert_eq!(start_json["protocol_version"], 1);
        assert_eq!(start_json["vocabulary"], "programming");
        assert_eq!(start_json["audio"]["encoding"], "pcm_s16le");
        assert_eq!(start_json["audio"]["channels"], 1);
        assert!(start_json.get("provider").is_none());
        assert!(start_json.get("api_key").is_none());
        assert_eq!(transport.frames.len(), 1);
        assert_eq!(transport.frames[0].sequence_id(), 1);
        assert_eq!(transport.frames[0].payload_len(), 8);
        let wire = transport.frames[0].clone().into_wire_bytes();
        assert_eq!(&wire[..8], &1u64.to_be_bytes());
        assert_eq!(&wire[8..10], &i16::MIN.to_le_bytes());

        assert_eq!(
            session.push_audio(&[0.0; 5]),
            Err(AsrErrorKind::Overflow),
            "an oversized callback is refused instead of split ambiguously"
        );
        assert_eq!(session.push_audio(&[f32::NAN]), Err(AsrErrorKind::Protocol));
        assert_eq!(session.telemetry().frames_queued, 1);
        assert_eq!(session.telemetry().samples_queued, 4);
    }

    #[test]
    fn normalized_receive_vocabulary_round_trips_without_vendor_fields() {
        let events = [
            partial("partial-1", 1, 1, "tekst"),
            final_event("final-1", 1, 2, "tekst final"),
            GatewayEvent::Error {
                event_id: "error-1".to_string(),
                session_id: session_id().to_string(),
                utterance_id: 1,
                code: GatewayErrorCode::RateLimited,
            },
            GatewayEvent::Usage {
                event_id: "usage-1".to_string(),
                session_id: session_id().to_string(),
                audio_ms: 500,
                billable_units: Some(1),
            },
            GatewayEvent::SessionEnded {
                session_id: session_id().to_string(),
            },
        ];
        for event in events {
            let encoded = serde_json::to_string(&event).expect("encode gateway event");
            let decoded: GatewayEvent =
                serde_json::from_str(&encoded).expect("decode gateway event");
            assert_eq!(decoded, event);
            assert!(!encoded.contains("provider"));
            assert!(!encoded.contains("api_key"));
        }

        let vendor_specific = r#"{
            "type":"transcript.partial",
            "event_id":"x",
            "session_id":"gateway-session-1",
            "utterance_id":1,
            "revision":1,
            "text":"x",
            "provider_model":"vendor-secret-shape"
        }"#;
        assert!(serde_json::from_str::<GatewayEvent>(vendor_specific).is_err());
    }

    #[test]
    fn voice_lab_wire_is_adapted_without_credential_fields() {
        let mut state = VoiceLabReceiveState::new(session_id().to_string());
        assert_eq!(
            state.adapt(r#"{"type":"ack","received_bytes":320}"#),
            Ok(None)
        );

        let partial = state
            .adapt(r#"{"type":"transcript.partial","text":"pierwszy"}"#)
            .expect("valid partial")
            .expect("partial event");
        assert!(matches!(
            partial,
            GatewayEvent::Partial {
                utterance_id: 1,
                revision: 1,
                ref text,
                ..
            } if text == "pierwszy"
        ));

        let final_event = state
            .adapt(r#"{"type":"transcript.final","text":"pierwszy final"}"#)
            .expect("valid final")
            .expect("final event");
        assert!(matches!(
            final_event,
            GatewayEvent::Final {
                utterance_id: 1,
                revision: 2,
                ref text,
                ..
            } if text == "pierwszy final"
        ));
        assert!(matches!(
            state
                .adapt(r#"{"type":"transcript.final","text":"drugi"}"#)
                .expect("second final")
                .expect("second event"),
            GatewayEvent::Final {
                utterance_id: 2,
                revision: 1,
                ..
            }
        ));
    }

    #[test]
    fn voice_lab_hello_and_control_frames_do_not_fault() {
        let mut state = VoiceLabReceiveState::new(session_id().to_string());
        assert_eq!(
            state.adapt(r#"{"type":"hello","protocol":"stt-ws-v1"}"#),
            Ok(None)
        );
        assert_eq!(
            state.adapt(r#"{"type":"speech.start","energy":0.4}"#),
            Ok(None)
        );
        assert_eq!(
            state.adapt(r#"{"type":"speech.end","energy":0.1}"#),
            Ok(None)
        );
        assert_eq!(
            state.adapt(r#"{"type":"vad.sample","energy":0.2,"is_speech":true}"#),
            Ok(None)
        );
        let empty = state
            .adapt(r#"{"type":"transcript.final","text":"","duration_ms":null}"#)
            .expect("empty final is still a final");
        assert!(
            matches!(empty, Some(GatewayEvent::Final { ref text, .. }) if text.is_empty()),
            "an empty final must reach the commit queue"
        );
        assert_eq!(state.adapt(r#"{"type":"future.control"}"#), Ok(None));
        assert!(matches!(
            state.adapt(r#"{"type":"stream.closed"}"#).expect("closed"),
            Some(GatewayEvent::SessionEnded { .. })
        ));
        assert!(matches!(
            state
                .adapt(r#"{"type":"transcript.final","text":"zostaje"}"#)
                .expect("final after hello")
                .expect("text"),
            GatewayEvent::Final {
                utterance_id: 2,
                revision: 1,
                ref text,
                ..
            } if text == "zostaje"
        ));
    }

    #[test]
    fn voice_lab_start_is_set_with_vocabulary_and_no_secret() {
        let payload: serde_json::Value = serde_json::from_str(&voice_lab_set_message(
            &GatewaySessionConfig::from_input(&input()),
        ))
        .expect("set json");
        assert_eq!(payload["type"], "set");
        assert_eq!(payload["language"], "pl-PL");
        assert_eq!(payload["sample_rate"], 16_000);
        assert_eq!(payload["encoding"], "pcm16");
        assert_eq!(payload["vocabulary"], "programming");
        assert_eq!(payload["vad"], false);
        assert!(payload.get("api_key").is_none());
        assert!(payload.get("type").and_then(|value| value.as_str()) != Some("config"));
    }

    #[test]
    fn local_sequence_is_global_across_reordered_utterances_and_duplicates() {
        let duplicate = partial("u2-r1", 2, 1, "drugi");
        let script = [
            GatewayTransportPoll::Event(partial("u1-r1", 1, 1, "pierwszy")),
            GatewayTransportPoll::Event(duplicate.clone()),
            GatewayTransportPoll::Event(duplicate),
            GatewayTransportPoll::Event(final_event("u1-r3", 1, 3, "pierwszy final")),
            GatewayTransportPoll::Event(partial("u1-r2-late", 1, 2, "spozniony")),
            GatewayTransportPoll::Event(final_event("u2-r2", 2, 2, "drugi final")),
            GatewayTransportPoll::Event(GatewayEvent::Usage {
                event_id: "usage-1".to_string(),
                session_id: session_id().to_string(),
                audio_ms: 1_250,
                billable_units: Some(2),
            }),
            GatewayTransportPoll::Pending,
        ];
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(script),
            limits(),
            authorization(),
        )
        .expect("session");
        session.open(&input()).expect("open");
        session.push_audio(&[0.0; 4]).expect("first span");
        session.commit(4).expect("first commit");
        session.push_audio(&[0.0; 4]).expect("second span");
        session.commit(8).expect("second commit");

        let events = session.drain();
        let sequences: Vec<_> = events.iter().map(AsrSessionEvent::sequence_id).collect();
        let utterances: Vec<_> = events.iter().map(AsrSessionEvent::utterance_id).collect();
        assert_eq!(sequences, vec![1, 2, 3, 4, 5]);
        assert_eq!(utterances, vec![1, 2, 1, 2, 0]);
        assert_eq!(events[2].as_token(), "final");
        assert_eq!(events[3].as_token(), "final");
        assert_eq!(events[4].as_token(), "usage");
        assert_eq!(session.telemetry().duplicate_events, 1);
        assert_eq!(session.telemetry().stale_events, 1);
    }

    #[test]
    fn delayed_transport_poll_never_blocks_drain() {
        let script = [
            GatewayTransportPoll::Pending,
            GatewayTransportPoll::Event(partial("delayed", 1, 1, "pozniej")),
            GatewayTransportPoll::Pending,
        ];
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(script),
            limits(),
            authorization(),
        )
        .expect("session");
        session.open(&input()).expect("open");
        assert!(session.drain().is_empty());
        assert_eq!(session.drain().len(), 1);
    }

    #[test]
    fn disconnect_is_a_typed_transport_event() {
        let script = [GatewayTransportPoll::Fault(AsrErrorKind::Transport)];
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(script),
            limits(),
            authorization(),
        )
        .expect("session");
        session.open(&input()).expect("open");
        let events = session.drain();
        assert!(matches!(
            events.as_slice(),
            [AsrSessionEvent::Error(ErrorEvent {
                kind: AsrErrorKind::Transport,
                ..
            })]
        ));
        assert_eq!(session.telemetry().transport_faults, 1);
    }

    #[test]
    fn auth_and_quota_are_distinct_content_free_events() {
        let script = [
            GatewayTransportPoll::Event(GatewayEvent::Error {
                event_id: "auth".to_string(),
                session_id: session_id().to_string(),
                utterance_id: 0,
                code: GatewayErrorCode::Auth,
            }),
            GatewayTransportPoll::Event(GatewayEvent::Error {
                event_id: "quota".to_string(),
                session_id: session_id().to_string(),
                utterance_id: 0,
                code: GatewayErrorCode::Quota,
            }),
            GatewayTransportPoll::Pending,
        ];
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(script),
            limits(),
            authorization(),
        )
        .expect("session");
        session.open(&input()).expect("open");
        let events = session.drain();
        assert!(matches!(
            &events[0],
            AsrSessionEvent::Error(ErrorEvent {
                kind: AsrErrorKind::Auth,
                ..
            })
        ));
        assert!(matches!(
            &events[1],
            AsrSessionEvent::Error(ErrorEvent {
                kind: AsrErrorKind::Quota,
                ..
            })
        ));
        assert_eq!(events[0].sequence_id(), 1);
        assert_eq!(events[1].sequence_id(), 2);
    }

    #[test]
    fn bounded_send_reports_backpressure_without_advancing_frame_sequence() {
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::with_send_capacity(1),
            limits(),
            authorization(),
        )
        .expect("session");
        session.open(&input()).expect("open");
        session.push_audio(&[0.0; 4]).expect("first frame");
        assert_eq!(session.push_audio(&[0.0; 4]), Err(AsrErrorKind::Overflow));
        assert_eq!(session.transport().frames.len(), 1);
        assert_eq!(session.transport().frames[0].sequence_id(), 1);
        assert_eq!(session.telemetry().backpressure_events, 1);
    }

    #[test]
    fn close_drains_trailing_final_and_usage_before_ack() {
        let script = [
            GatewayTransportPoll::Event(final_event("tail", 2, 7, "ogon")),
            GatewayTransportPoll::Event(GatewayEvent::Usage {
                event_id: "tail-usage".to_string(),
                session_id: session_id().to_string(),
                audio_ms: 2_000,
                billable_units: None,
            }),
            GatewayTransportPoll::Event(GatewayEvent::SessionEnded {
                session_id: session_id().to_string(),
            }),
        ];
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(script),
            limits(),
            authorization(),
        )
        .expect("session");
        session.open(&input()).expect("open");
        session
            .push_audio(&[0.0; 4])
            .expect("audio for the end commit");
        session.close().expect("bounded close");
        let events = session.drain();
        assert_eq!(events.len(), 2);
        assert!(events[0].is_final());
        assert_eq!(events[1].as_token(), "usage");
    }

    #[test]
    fn close_timeout_aborts_and_emits_one_typed_fault() {
        let mut short_limits = limits();
        short_limits.close_timeout = Duration::from_millis(1);
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::default(),
            short_limits,
            authorization(),
        )
        .expect("session");
        session.open(&input()).expect("open");
        assert_eq!(session.close(), Err(AsrErrorKind::Transport));
        assert!(session.transport().aborted);
        let events = session.drain();
        assert!(matches!(
            events.as_slice(),
            [AsrSessionEvent::Error(ErrorEvent {
                kind: AsrErrorKind::Transport,
                ..
            })]
        ));
    }

    #[test]
    fn connection_and_telemetry_debug_are_secret_safe() {
        let connection = GatewayConnection::new(
            "wss://gateway.invalid/live?signed=do-not-log",
            "bearer-do-not-log",
        )
        .expect("valid normalized gateway");
        let debug = format!("{connection:?}");
        assert!(!debug.contains("signed=do-not-log"));
        assert!(!debug.contains("bearer-do-not-log"));
        assert!(debug.contains("REDACTED"));

        let telemetry = format!("{:?}", CloudSessionTelemetry::default());
        assert!(!telemetry.contains("gateway.invalid"));
        assert!(!telemetry.contains("bearer"));
    }

    #[test]
    fn production_connection_refuses_remote_plaintext_and_user_info() {
        let plain_websocket = concat!("ws", "://");
        assert_eq!(
            GatewayConnection::new(format!("{plain_websocket}gateway.invalid/live"), "token")
                .unwrap_err(),
            AsrErrorKind::Protocol
        );
        assert_eq!(
            GatewayConnection::new("wss://user@gateway.invalid/live", "token").unwrap_err(),
            AsrErrorKind::Protocol
        );
        assert!(
            GatewayConnection::new(format!("{plain_websocket}127.0.0.1:9000/live"), "token")
                .is_ok()
        );
        assert!(
            GatewayConnection::new(format!("{plain_websocket}127.0.0.1:9000/live"), "").is_ok(),
            "loopback live STT must not require a key"
        );
    }

    #[test]
    fn voice_lab_loopback_hello_keeps_the_session_open() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let (first_tx, first_rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let Ok(mut socket) = tokio_tungstenite::tungstenite::accept(stream) else {
                return;
            };
            let hello = Message::Text(r#"{"type":"hello","protocol":"stt-ws-v1"}"#.into());
            if socket.send(hello).is_err() {
                return;
            }
            if let Ok(Message::Text(text)) = socket.read() {
                let _ = first_tx.send(text.to_string());
                let _ = socket.send(Message::Text(r#"{"type":"ack"}"#.into()));
                let _ = socket.send(Message::Text(
                    r#"{"type":"speech.start","energy":0.5}"#.into(),
                ));
            }
            while socket.read().is_ok() {}
        });

        let endpoint = format!("{}{addr}/v1/audio/transcribe", concat!("ws", "://"));
        let limits = CloudSessionLimits {
            connect_timeout: Duration::from_secs(2),
            send_timeout: Duration::from_secs(1),
            close_timeout: Duration::from_millis(200),
            ..CloudSessionLimits::default()
        };
        let connection = GatewayConnection::new(endpoint, "").expect("loopback connection");
        let transport = GatewayWebSocketTransport::new(connection, limits).expect("transport");
        let mut session =
            LiveCloudAsrSession::new(transport, limits, authorization()).expect("session");
        session.open(&input()).expect("open");

        let first = first_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("Voice Lab start frame");
        let start: serde_json::Value = serde_json::from_str(&first).expect("start json");
        assert_eq!(start["type"], "set");
        assert_ne!(start["type"], "config");

        for _ in 0..20 {
            let events = session.drain();
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event, AsrSessionEvent::Error(_))),
                "hello/control must not degrade the live lane: {events:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        session
            .push_audio(&[0.0; 4])
            .expect("PCM after hello stays accepted");
        let _ = session.close();
    }

    fn input_hz(rate: u32) -> SessionInput {
        SessionInput {
            session_id: session_id(),
            locale: Some("pl-PL".to_string()),
            sample_rate: rate,
        }
    }

    fn wide_limits(frame: usize, outbound: usize, connect: Duration) -> CloudSessionLimits {
        CloudSessionLimits {
            max_frame_samples: frame,
            max_events_per_drain: 32,
            max_close_events: 16,
            outbound_queue_capacity: outbound,
            inbound_queue_capacity: 16,
            remembered_event_ids: 32,
            connect_timeout: connect,
            send_timeout: Duration::from_secs(2),
            close_timeout: Duration::from_secs(2),
        }
    }

    fn push_samples(
        session: &mut LiveCloudAsrSession<FakeGatewayTransport>,
        total: usize,
        frame: usize,
    ) {
        let mut left = total;
        while left > 0 {
            let count = left.min(frame);
            session
                .push_audio(&vec![0.0; count])
                .expect("capture frame");
            left -= count;
        }
    }

    fn capture_bounds(range: AudioRange, rate_hz: u32) -> (u64, u64) {
        let rate = f64::from(rate_hz);
        (
            (f64::from(range.start_secs()) * rate).round() as u64,
            (f64::from(range.end_secs()) * rate).round() as u64,
        )
    }

    fn final_bounds(event: &AsrSessionEvent, rate_hz: u32) -> (String, (u64, u64)) {
        let AsrSessionEvent::Final(transcript) = event else {
            panic!("expected a final, got {event:?}");
        };
        let range = transcript.range.expect("final carries its commit span");
        (transcript.text.clone(), capture_bounds(range, rate_hz))
    }

    fn loopback_endpoint(addr: std::net::SocketAddr) -> String {
        format!("{}{addr}/v1/audio/transcribe", concat!("ws", "://"))
    }

    #[test]
    fn preconnect_capacity_covers_connect_timeout_at_192_khz() {
        let window = Duration::from_millis(500);
        let capacity = preconnect_sample_capacity(window);
        let native_budget = 192_000 * 500 / 1_000;
        assert!(capacity >= native_budget);
        let frames = 500 / 10;
        let frame_samples = 48_000 * 10 / 1_000;
        assert!(frames * frame_samples <= capacity);

        let mut buffer = PreconnectBuffer::covering(Duration::from_millis(10));
        let fitting = GatewayPcmFrame {
            sequence_id: 1,
            pcm_s16le: vec![0; 1_920 * 2],
        };
        buffer.push_pcm(fitting).expect("10 ms at 192 kHz fits");
        let extra = GatewayPcmFrame {
            sequence_id: 2,
            pcm_s16le: vec![0; 2],
        };
        assert_eq!(buffer.push_pcm(extra), Err(AsrErrorKind::Transport));
    }

    #[test]
    fn handshake_buffers_native_frames_until_connect_then_delivers_in_order() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (report_tx, report_rx) = std::sync::mpsc::channel::<(bool, u64, Vec<i16>)>();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            let Ok(mut socket) = tokio_tungstenite::tungstenite::accept(stream) else {
                return;
            };
            let Ok(Message::Text(set_text)) = socket.read() else {
                return;
            };
            let set: serde_json::Value = serde_json::from_str(&set_text).unwrap_or_default();
            let vad_off = set.get("vad").and_then(serde_json::Value::as_bool) == Some(false);
            let sample_rate = set
                .get("sample_rate")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let mut markers = Vec::new();
            while markers.len() < 50 {
                let Ok(Message::Text(text)) = socket.read() else {
                    break;
                };
                let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                if value.get("type").and_then(serde_json::Value::as_str) != Some("chunk") {
                    continue;
                }
                let Some(encoded) = value
                    .get("audio_base64")
                    .and_then(serde_json::Value::as_str)
                else {
                    break;
                };
                let Ok(bytes) = BASE64.decode(encoded) else {
                    break;
                };
                let marker = bytes
                    .chunks_exact(2)
                    .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                    .take_while(|sample| *sample == i16::MAX)
                    .count() as i16;
                markers.push(marker);
            }
            let _ = report_tx.send((vad_off, sample_rate, markers));
        });

        let limits = wide_limits(480, 8, Duration::from_secs(2));
        let connection = GatewayConnection::new(loopback_endpoint(addr), "").expect("endpoint");
        let transport = GatewayWebSocketTransport::new(connection, limits).expect("transport");
        let mut session =
            LiveCloudAsrSession::new(transport, limits, authorization()).expect("session");
        session.open(&input_hz(48_000)).expect("open");
        for index in 1..=50u16 {
            let mut samples = vec![0.0; 480];
            for sample in samples.iter_mut().take(usize::from(index)) {
                *sample = 1.0;
            }
            session
                .push_audio(&samples)
                .expect("handshake frame is accepted");
        }

        let (vad_off, sample_rate, markers) = report_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("server saw the buffered frames");
        assert!(vad_off, "set must carry vad:false");
        assert_eq!(sample_rate, 48_000);
        assert_eq!(
            markers,
            (1..=50).map(|index| index as i16).collect::<Vec<_>>()
        );
        assert_eq!(session.telemetry().frames_queued, 50);
        assert_eq!(session.telemetry().backpressure_events, 0);
        assert!(
            session
                .drain()
                .iter()
                .all(|event| !matches!(event, AsrSessionEvent::Error(_)))
        );
    }

    #[test]
    fn connect_timeout_degrades_as_disconnect_not_overflow() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let limits = wide_limits(480, 8, Duration::from_millis(200));
        let connection = GatewayConnection::new(loopback_endpoint(addr), "").expect("endpoint");
        let transport = GatewayWebSocketTransport::new(connection, limits).expect("transport");
        let mut session =
            LiveCloudAsrSession::new(transport, limits, authorization()).expect("session");
        session.open(&input_hz(48_000)).expect("open");
        for _ in 0..20 {
            session
                .push_audio(&vec![0.0; 480])
                .expect("frames during connect are buffered");
        }
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            session.push_audio(&vec![0.0; 480]),
            Err(AsrErrorKind::Transport)
        );
        assert_eq!(session.telemetry().backpressure_events, 0);
        let events = session.drain();
        assert!(
            matches!(
                events.as_slice(),
                [AsrSessionEvent::Error(ErrorEvent {
                    kind: AsrErrorKind::Transport,
                    ..
                })]
            ),
            "timeout must surface transport, got {events:?}"
        );
        drop(listener);
    }

    #[test]
    fn stalled_socket_after_connect_still_overflows_the_live_queue() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (set_tx, set_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let Ok(mut socket) = tokio_tungstenite::tungstenite::accept(stream) else {
                return;
            };
            if socket.read().is_ok() {
                let _ = set_tx.send(());
            }
            std::thread::sleep(Duration::from_secs(2));
        });

        let limits = wide_limits(38_400, 2, Duration::from_secs(2));
        let connection = GatewayConnection::new(loopback_endpoint(addr), "").expect("endpoint");
        let transport = GatewayWebSocketTransport::new(connection, limits).expect("transport");
        let mut session =
            LiveCloudAsrSession::new(transport, limits, authorization()).expect("session");
        session.open(&input_hz(48_000)).expect("open");
        set_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("handshake completed");

        let mut overflowed = false;
        for _ in 0..80 {
            match session.push_audio(&vec![0.2; 38_400]) {
                Ok(()) => {}
                Err(AsrErrorKind::Overflow) => {
                    overflowed = true;
                    break;
                }
                Err(other) => panic!("post-connect stall must be overflow, got {other}"),
            }
        }
        assert!(overflowed, "a stalled live queue must still overflow");
        assert!(session.telemetry().backpressure_events >= 1);
    }

    #[test]
    fn commits_stamp_finals_on_the_capture_clock_including_empty_and_unanchored() {
        let rate = 48_000u32;
        let mut limits = wide_limits(48_000, 8, Duration::from_secs(2));
        limits.close_timeout = Duration::from_millis(200);
        let script = [
            GatewayTransportPoll::Event(final_event("f1", 1, 1, "raz")),
            GatewayTransportPoll::Event(final_event("f2", 2, 1, "dwa")),
            GatewayTransportPoll::Event(final_event("f3", 3, 1, "trzy")),
            GatewayTransportPoll::Event(GatewayEvent::SessionEnded {
                session_id: session_id().to_string(),
            }),
        ];
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(script),
            limits,
            authorization(),
        )
        .expect("session");
        session.open(&input_hz(rate)).expect("open");
        push_samples(&mut session, 1_152_000, 48_000);
        session.commit(384_000).expect("first commit");
        session.commit(768_000).expect("second commit");
        session.close().expect("end is the third commit");
        let events = session.drain();
        let stamped: Vec<_> = events
            .iter()
            .map(|event| final_bounds(event, rate))
            .collect();
        assert_eq!(
            stamped,
            vec![
                ("raz".to_string(), (0, 384_000)),
                ("dwa".to_string(), (384_000, 768_000)),
                ("trzy".to_string(), (768_000, 1_152_000)),
            ]
        );
        assert!(session.transport().ending);

        let empty_script = [
            GatewayTransportPoll::Event(final_event("word", 1, 1, "halo")),
            GatewayTransportPoll::Event(final_event("empty", 2, 1, "")),
        ];
        let mut empty_session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(empty_script),
            limits,
            authorization(),
        )
        .expect("session");
        empty_session.open(&input_hz(rate)).expect("open");
        push_samples(&mut empty_session, 200, 200);
        empty_session.commit(100).expect("first");
        empty_session.commit(200).expect("second");
        let empty_events = empty_session.drain();
        assert_eq!(final_bounds(&empty_events[0], rate).1, (0, 100));
        assert_eq!(final_bounds(&empty_events[1], rate).0, "");
        assert_eq!(final_bounds(&empty_events[1], rate).1, (100, 200));

        let unanchored = [GatewayTransportPoll::Event(final_event(
            "loose", 1, 1, "bez",
        ))];
        let mut bare = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(unanchored),
            limits,
            authorization(),
        )
        .expect("session");
        bare.open(&input_hz(rate)).expect("open");
        let faults = bare.drain();
        assert!(
            matches!(
                faults.as_slice(),
                [AsrSessionEvent::Error(ErrorEvent {
                    kind: AsrErrorKind::Protocol,
                    ..
                })]
            ),
            "unanchored final is a protocol error, got {faults:?}"
        );
        assert!(faults.iter().all(|event| !event.is_final()));
    }

    #[test]
    fn probe_vad_false_wire_replays_into_three_stamped_finals() {
        // Shape measured on the 25 IX `vad:false` probe: `hello`, one
        // `transcript.final` per flush, one for `end`, then `stream.closed`.
        // Each final carries `duration_ms: null` and a `response_id`. The
        // byte log was not in this worktree; the texts below are stand-ins.
        let lines = [
            r#"{"type":"hello","protocol":"stt-ws-v1"}"#,
            r#"{"type":"transcript.final","text":"raz","duration_ms":null,"response_id":"r1"}"#,
            r#"{"type":"transcript.final","text":"dwa","duration_ms":null,"response_id":"r2"}"#,
            r#"{"type":"transcript.final","text":"trzy","duration_ms":null,"response_id":"r3"}"#,
            r#"{"type":"stream.closed"}"#,
        ];
        let mut state = VoiceLabReceiveState::new(session_id().to_string());
        let mut script = Vec::new();
        for line in lines {
            if let Some(event) = state.adapt(line).expect("probe line") {
                script.push(GatewayTransportPoll::Event(event));
            }
        }
        assert_eq!(
            script.len(),
            4,
            "hello is ignored; three finals plus closed"
        );

        let rate = 48_000u32;
        let mut limits = wide_limits(48_000, 8, Duration::from_secs(2));
        limits.close_timeout = Duration::from_millis(200);
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(script),
            limits,
            authorization(),
        )
        .expect("session");
        session.open(&input_hz(rate)).expect("open");
        push_samples(&mut session, 1_152_000, 48_000);
        session.commit(384_000).expect("flush at 8s");
        session.commit(768_000).expect("flush at 16s");
        session.close().expect("end at 24s");
        let events = session.drain();
        let stamped: Vec<_> = events
            .iter()
            .filter(|event| event.is_final())
            .map(|event| final_bounds(event, rate).1)
            .collect();
        assert_eq!(
            stamped,
            vec![(0, 384_000), (384_000, 768_000), (768_000, 1_152_000)]
        );
    }

    #[test]
    fn echoed_item_id_wins_over_fifo_order() {
        let rate = 48_000u32;
        let limits = wide_limits(200, 8, Duration::from_secs(2));
        let script = [
            GatewayTransportPoll::Event(final_with_item(
                "second-first",
                1,
                "pozniej",
                "cs-commit-2",
            )),
            GatewayTransportPoll::Event(final_event("first-later", 2, 1, "wczesniej")),
        ];
        let mut session = LiveCloudAsrSession::new(
            FakeGatewayTransport::scripted(script),
            limits,
            authorization(),
        )
        .expect("session");
        session.open(&input_hz(rate)).expect("open");
        push_samples(&mut session, 200, 200);
        session.commit(100).expect("cs-commit-1");
        session.commit(200).expect("cs-commit-2");
        let events = session.drain();
        assert_eq!(final_bounds(&events[0], rate).1, (100, 200));
        assert_eq!(final_bounds(&events[1], rate).1, (0, 100));
    }
}
