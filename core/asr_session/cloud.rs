//! Dedicated live cloud transport for the Libraxis Voice Lab WebSocket.
//!
//! This module owns normal live capture: a `config` message, bounded base64 PCM
//! `chunk` messages, periodic `flush`, and a bounded `end`/drain. The receive
//! adapter converts Voice Lab events into Codescribe's normalized vocabulary.
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
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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
                && credential.trim().is_empty())
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
}

impl VoiceLabReceiveState {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            next_event_id: 1,
            utterance_id: 1,
            revision: 0,
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

    fn adapt(&mut self, text: &str) -> Result<Option<GatewayEvent>, AsrErrorKind> {
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
                let Some(text) = voice_lab_text(&value) else {
                    return Ok(None);
                };
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

/// Voice Lab live start frame. The engine's frozen inbound types are
/// `set` / `chunk` / `flush` / `end` — `config` is rejected as unknown.
fn voice_lab_set_message(config: &GatewaySessionConfig) -> String {
    serde_json::json!({
        "type": "set",
        "language": config.locale.as_deref().unwrap_or("pl"),
        "sample_rate": config.sample_rate_hz(),
        "encoding": "pcm16",
        "vocabulary": config.vocabulary,
    })
    .to_string()
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
    End,
    Abort,
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
        Ok(Self {
            connection: Some(connection),
            limits,
            command_tx: None,
            event_rx: None,
            worker: None,
            started: false,
            ending: false,
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
        let worker = std::thread::Builder::new()
            .name("codescribe-live-cloud-asr".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    let _ = event_tx.blocking_send(WorkerSignal::Fault(AsrErrorKind::Transport));
                    let _ = event_tx.blocking_send(WorkerSignal::Closed);
                    return;
                };
                runtime.block_on(gateway_worker(
                    connection, config, limits, command_rx, event_tx,
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
        let sender = self.command_tx.as_ref().ok_or(AsrErrorKind::Transport)?;
        sender
            .try_send(GatewayCommand::Pcm(frame))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AsrErrorKind::Overflow,
                mpsc::error::TrySendError::Closed(_) => AsrErrorKind::Transport,
            })
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
        let sender = self.command_tx.as_ref().ok_or(AsrErrorKind::Transport)?;
        sender
            .try_send(GatewayCommand::End)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AsrErrorKind::Overflow,
                mpsc::error::TrySendError::Closed(_) => AsrErrorKind::Transport,
            })?;
        self.ending = true;
        Ok(())
    }

    fn abort(&mut self) {
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
) {
    let result = run_gateway_socket(connection, config, limits, command_rx, &event_tx).await;
    if let Err(kind) = result {
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
) -> Result<(), AsrErrorKind> {
    let mut request = connection
        .endpoint
        .as_str()
        .into_client_request()
        .map_err(|_| AsrErrorKind::Protocol)?;
    match connection.auth_mode {
        crate::stt::tail_provider::SttAuthMode::Unauthenticated => {}
        crate::stt::tail_provider::SttAuthMode::Bearer => {
            let authorization =
                HeaderValue::from_str(&format!("Bearer {}", connection.credential.trim()))
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

    let connected = timeout(limits.connect_timeout, connect_async(request))
        .await
        .map_err(|_| AsrErrorKind::Transport)?
        .map_err(|error| classify_socket_error(&error))?;
    let (mut socket, _) = connected;

    // Proven Voice Lab wire: credentials stay in the WebSocket handshake,
    // never in the JSON body. The engine start type is `set`, not `config`.
    send_socket_message(
        &mut socket,
        Message::Text(voice_lab_set_message(&config).into()),
        limits.send_timeout,
    )
    .await?;

    let mut receive_state = VoiceLabReceiveState::new(config.session_id.clone());
    let mut flush = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_millis(2_500),
        Duration::from_millis(2_500),
    );
    loop {
        tokio::select! {
            command = command_rx.recv() => {
                match command {
                    Some(GatewayCommand::Pcm(frame)) => {
                        let chunk = serde_json::json!({
                            "type": "chunk",
                            "audio_base64": BASE64.encode(&frame.pcm_s16le),
                            "sample_rate": config.audio.sample_rate_hz,
                            "encoding": "pcm16",
                        }).to_string();
                        send_socket_message(
                            &mut socket,
                            Message::Text(chunk.into()),
                            limits.send_timeout,
                        ).await?;
                    }
                    Some(GatewayCommand::End) => {
                        let flush = serde_json::json!({"type": "flush"}).to_string();
                        send_socket_message(
                            &mut socket,
                            Message::Text(flush.into()),
                            limits.send_timeout,
                        ).await?;
                        let end = serde_json::json!({"type": "end"}).to_string();
                        send_socket_message(
                            &mut socket,
                            Message::Text(end.into()),
                            limits.send_timeout,
                        ).await?;
                        return drain_gateway_tail(
                            &mut socket,
                            limits,
                            event_tx,
                            &mut receive_state,
                        ).await;
                    }
                    Some(GatewayCommand::Abort) | None => {
                        let _ = socket.close(None).await;
                        return Ok(());
                    }
                }
            }
            _ = flush.tick() => {
                let message = serde_json::json!({"type": "flush"}).to_string();
                send_socket_message(
                    &mut socket,
                    Message::Text(message.into()),
                    limits.send_timeout,
                ).await?;
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
            return Ok(());
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
pub struct LiveCloudAsrSession<T: CloudGatewayTransport> {
    _authorization: CloudEgressAuthorization,
    transport: T,
    limits: CloudSessionLimits,
    state: SessionState,
    session_id: Option<SessionId>,
    next_audio_sequence: u64,
    next_event_sequence: u64,
    utterance_revisions: HashMap<u64, u64>,
    sealed_utterances: HashSet<u64>,
    seen_event_ids: SeenEventIds,
    ready: VecDeque<AsrSessionEvent>,
    telemetry: CloudSessionTelemetry,
    fault_seen: bool,
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
            } => self.normalize_transcript(false, utterance_id, revision, text, start_ms, end_ms),
            GatewayEvent::Final {
                utterance_id,
                revision,
                text,
                start_ms,
                end_ms,
                ..
            } => self.normalize_transcript(true, utterance_id, revision, text, start_ms, end_ms),
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

    fn normalize_transcript(
        &mut self,
        is_final: bool,
        utterance_id: u64,
        revision: u64,
        text: String,
        start_ms: Option<u64>,
        end_ms: Option<u64>,
    ) -> Result<Option<AsrSessionEvent>, AsrErrorKind> {
        if text.trim().is_empty() {
            return Err(AsrErrorKind::Protocol);
        }
        if self.sealed_utterances.contains(&utterance_id)
            || self
                .utterance_revisions
                .get(&utterance_id)
                .is_some_and(|previous| revision <= *previous)
        {
            self.telemetry.stale_events += 1;
            return Ok(None);
        }
        let range = match (start_ms, end_ms) {
            (None, None) => None,
            (Some(start), Some(end)) => Some(
                AudioRange::new(duration_millis_to_secs(start), duration_millis_to_secs(end))
                    .ok_or(AsrErrorKind::Protocol)?,
            ),
            _ => return Err(AsrErrorKind::Protocol),
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
        if let Err(kind) = self.transport.begin_end() {
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
        }
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
        assert_eq!(
            state.adapt(r#"{"type":"transcript.final","text":""}"#),
            Ok(None)
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
                utterance_id: 1,
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

        let events = session.drain();
        let sequences: Vec<_> = events
            .iter()
            .map(AsrSessionEvent::sequence_id)
            .collect();
        let utterances: Vec<_> = events
            .iter()
            .map(AsrSessionEvent::utterance_id)
            .collect();
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
}
