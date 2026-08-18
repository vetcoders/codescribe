//! Typed provider contract for bounded Whisper tail-patch windows.
//!
//! Time on this seam is always an integer PCM sample range. Floating-point
//! seconds exist only in the adapter back to the legacy [`TranscriptSegment`]
//! surface. The in-process implementation, localhost WebSocket sidecar, and
//! remote multipart client all terminate on this one seam. Sidecar/remote
//! failures fall back without changing the caller's append-only contract.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use rand::RngCore;
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::blocking::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::{Message, accept, client};

use crate::pipeline::contracts::{RawTranscript, TranscriptSegment};

/// Environment key selecting the tail-patch provider.
pub const STT_TAIL_PROVIDER_ENV: &str = "STT_TAIL_PROVIDER";
/// Optional development override for the sidecar executable.
pub const STT_SIDECAR_BIN_ENV: &str = "CODESCRIBE_STT_SIDECAR_BIN";
/// Child-only authentication token; never read from operator config.
pub const STT_SIDECAR_TOKEN_ENV: &str = "CODESCRIBE_STT_SIDECAR_TOKEN";

/// Authentication header required by a multipart STT endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttAuthMode {
    /// Loopback transcription servers are host-local and need no API key.
    Unauthenticated,
    /// Official/vendor endpoints authenticate API keys as bearer tokens.
    Bearer,
    /// Custom non-loopback endpoints retain the historical `x-api-key` contract.
    ApiKey,
}

/// Resolve STT authentication from the endpoint owner.
///
/// OpenAI and Libraxis both expose OpenAI-compatible multipart transcription
/// endpoints authenticated with `Authorization: Bearer`. Loopback endpoints
/// need no API key. Unknown custom endpoints preserve the existing `x-api-key`
/// contract until provider-registry v2 carries an explicit auth mode.
pub fn stt_auth_mode(endpoint: &str) -> SttAuthMode {
    let host = Url::parse(endpoint).ok().and_then(|url| {
        url.host_str()
            .map(|host| host.trim_matches(['[', ']']).to_owned())
    });
    match host.as_deref() {
        Some(host)
            if host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback()) =>
        {
            SttAuthMode::Unauthenticated
        }
        Some(host)
            if host.eq_ignore_ascii_case("api.openai.com")
                || host.eq_ignore_ascii_case("api.libraxis.cloud") =>
        {
            SttAuthMode::Bearer
        }
        _ => SttAuthMode::ApiKey,
    }
}

/// Map a live WebSocket STT URL onto the multipart file probe.
///
/// Settings → Test is always OpenAI-compatible `POST /v1/audio/transcriptions`.
/// `http`/`https` stay. `ws`/`wss` whose path ends in `/transcribe` invert
/// scheme and path — same rewrite for every host. Loopback `:8446` (Voice Lab
/// socket) becomes `:8444` (file worker). Other live sockets stay as-is so
/// [`validate_remote_endpoint`] still fail-closes them.
pub(crate) fn file_probe_endpoint(endpoint: &str) -> String {
    let Ok(mut url) = Url::parse(endpoint) else {
        return endpoint.to_string();
    };
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .to_owned();
    match url.scheme() {
        "http" | "https" => return url.to_string(),
        "ws" | "wss" => {}
        _ => return endpoint.to_string(),
    }

    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let http_scheme = if url.scheme() == "wss" {
        "https"
    } else {
        "http"
    };

    if !url.path().ends_with("/transcribe") {
        return endpoint.to_string();
    }
    if url.set_scheme(http_scheme).is_err() {
        return endpoint.to_string();
    }
    let path = url.path().trim_end_matches("transcribe").to_string() + "transcriptions";
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    if loopback && url.port() == Some(8446) && url.set_port(Some(8444)).is_err() {
        return endpoint.to_string();
    }
    url.to_string()
}

const SIDECAR_PROTOCOL_VERSION: u8 = 1;
const SIDECAR_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SIDECAR_IO_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_LOCAL_REMOTE_ENDPOINT: &str = "http://127.0.0.1:8000/v1/audio/transcriptions";
const MAX_TAIL_PROVIDER_PCM_BYTES: usize = 32 * 1024 * 1024;

/// Maximum transcript bytes accepted across the provider seam.
pub const MAX_TAIL_PROVIDER_TEXT_BYTES: usize = 64 * 1024;
/// Maximum timed segments accepted across the provider seam.
pub const MAX_TAIL_PROVIDER_SEGMENTS: usize = 2_048;
/// Maximum bytes accepted for an identity/session or evidence revision token.
pub const MAX_TAIL_PROVIDER_ID_BYTES: usize = 256;

/// Compatibility request counter until W13-3A threads capture identity into
/// the live call site. It prevents unrelated legacy windows from sharing an
/// idempotency key; explicit callers should supply their own identity.
static LEGACY_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Provider incarnation chosen for a tail-patch request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TailProviderId {
    InProcess,
    Sidecar,
    Remote,
    Fake,
}

impl TailProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProcess => "inprocess",
            Self::Sidecar => "sidecar",
            Self::Remote => "remote",
            Self::Fake => "fake",
        }
    }

    /// Parse the provider selector without changing the default.
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "inprocess" | "in_process" => Ok(Self::InProcess),
            "sidecar" => Ok(Self::Sidecar),
            "remote" => Ok(Self::Remote),
            other => bail!(
                "invalid {STT_TAIL_PROVIDER_ENV} value {other:?}; expected inprocess, sidecar, or remote"
            ),
        }
    }
}

/// Canonical identity of one PCM range in one capture epoch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TailSampleRange {
    pub session: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
}

impl TailSampleRange {
    pub fn sample_len(&self) -> Result<u64> {
        self.sample_end
            .checked_sub(self.sample_start)
            .ok_or_else(|| {
                anyhow!(
                    "tail range ends before it starts: {}..{}",
                    self.sample_start,
                    self.sample_end
                )
            })
    }

    fn contains(&self, other: &Self) -> bool {
        self.session == other.session
            && self.capture_epoch == other.capture_epoch
            && self.sample_start <= other.sample_start
            && other.sample_end <= self.sample_end
    }
}

/// Idempotency key plus the exact audio range it names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TailRequestIdentity {
    pub request_id: u64,
    pub range: TailSampleRange,
}

/// Typed stability of the provider evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TailEvidenceStability {
    Final,
}

/// Honesty label for the timestamp mapping carried by a payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TailTimingQuality {
    /// Segment ranges are exact on the capture PCM clock.
    ExactSampleRange,
    /// Current in-process Whisper timestamps refer to VAD-compacted speech.
    CompactedSpeechRelative,
    /// Deterministic test evidence, not a measured engine timestamp.
    Synthetic,
}

impl TailTimingQuality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactSampleRange => "exact_sample_range",
            Self::CompactedSpeechRelative => "compacted_speech_relative",
            Self::Synthetic => "synthetic",
        }
    }
}

/// Engine family that produced the evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TailEvidenceSource {
    AppleSpeech,
    Whisper,
}

impl TailEvidenceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppleSpeech => "apple_speech",
            Self::Whisper => "whisper",
        }
    }
}

/// Provenance fields that must exist before confidence participates in fusion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TailProviderEvidence {
    pub source: TailEvidenceSource,
    pub revision: Option<String>,
    pub stability: TailEvidenceStability,
    pub timing_quality: TailTimingQuality,
    /// Raw engine confidence; never calibrated or promoted on this seam.
    pub avg_logprob: Option<f32>,
}

/// One provider segment pinned to the canonical sample clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedTailSegment {
    pub text: String,
    pub range: TailSampleRange,
}

/// Bounded, typed result returned by every provider incarnation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TailProviderPayload {
    pub identity: TailRequestIdentity,
    pub text: String,
    pub segments: Vec<TimedTailSegment>,
    pub avg_logprob: Option<f32>,
    pub compression_ratio: Option<f32>,
    pub quality_gate_dropped: bool,
    pub provider_id: TailProviderId,
    pub elapsed_ms: u64,
    pub evidence: TailProviderEvidence,
}

impl TailProviderPayload {
    /// Enforce transport bounds and range identity before the payload can reach
    /// fusion code.
    pub fn validate(&self) -> Result<()> {
        if self.identity.range.session.trim().is_empty() {
            bail!("tail provider session identity must be non-empty");
        }
        if self.identity.range.session.len() > MAX_TAIL_PROVIDER_ID_BYTES {
            bail!("tail provider session identity is too long");
        }
        if self.text.len() > MAX_TAIL_PROVIDER_TEXT_BYTES {
            bail!("tail provider text exceeds {MAX_TAIL_PROVIDER_TEXT_BYTES} bytes");
        }
        if self.segments.len() > MAX_TAIL_PROVIDER_SEGMENTS {
            bail!("tail provider returned too many segments");
        }
        self.identity.range.sample_len()?;
        let mut segment_text_bytes = 0usize;
        for segment in &self.segments {
            segment.range.sample_len()?;
            if !self.identity.range.contains(&segment.range) {
                bail!("tail provider segment range escapes request range");
            }
            segment_text_bytes = segment_text_bytes
                .checked_add(segment.text.len())
                .ok_or_else(|| anyhow!("tail provider segment text size overflow"))?;
            if segment_text_bytes > MAX_TAIL_PROVIDER_TEXT_BYTES {
                bail!("tail provider segment text exceeds bounded payload size");
            }
        }
        if self
            .evidence
            .revision
            .as_ref()
            .is_some_and(|revision| revision.len() > MAX_TAIL_PROVIDER_ID_BYTES)
        {
            bail!("tail provider evidence revision is too long");
        }
        if self.avg_logprob.is_some_and(|value| !value.is_finite())
            || self
                .evidence
                .avg_logprob
                .is_some_and(|value| !value.is_finite())
        {
            bail!("tail provider avg_logprob must be finite");
        }
        if self.avg_logprob != self.evidence.avg_logprob {
            bail!("tail provider confidence disagrees with typed evidence");
        }
        Ok(())
    }

    /// Adapter back to the legacy seconds-based pipeline contract.
    pub fn into_raw_transcript(self, sample_rate: u32) -> Result<RawTranscript> {
        if sample_rate == 0 {
            bail!("tail provider sample_rate must be non-zero");
        }
        self.validate()?;
        let request_start = self.identity.range.sample_start;
        let rate = sample_rate as f64;
        Ok(RawTranscript {
            text: self.text,
            segments: self
                .segments
                .into_iter()
                .map(|segment| TranscriptSegment {
                    text: segment.text,
                    start_ts: ((segment.range.sample_start - request_start) as f64 / rate) as f32,
                    end_ts: ((segment.range.sample_end - request_start) as f64 / rate) as f32,
                })
                .collect(),
            avg_logprob: self.avg_logprob,
            compression_ratio: self.compression_ratio,
            quality_gate_dropped: self.quality_gate_dropped,
        })
    }
}

/// Metadata for one bounded transcription request. PCM stays borrowed and is
/// passed separately so the in-process refit does not copy every live window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailProviderRequest {
    pub identity: TailRequestIdentity,
    pub sample_rate: u32,
    pub language: Option<String>,
}

impl TailProviderRequest {
    pub fn validate_pcm(&self, pcm: &[f32]) -> Result<()> {
        if self.sample_rate == 0 {
            bail!("tail provider sample_rate must be non-zero");
        }
        if self
            .language
            .as_ref()
            .is_some_and(|language| language.len() > MAX_TAIL_PROVIDER_ID_BYTES)
        {
            bail!("tail provider language token is too long");
        }
        let expected = self.identity.range.sample_len()?;
        if expected != pcm.len() as u64 {
            bail!(
                "tail request range length {expected} does not match PCM length {}",
                pcm.len()
            );
        }
        Ok(())
    }
}

/// One transport-neutral tail-patch provider.
pub trait TailProvider: Send + Sync {
    fn provider_id(&self) -> TailProviderId;
    fn transcribe(&self, request: &TailProviderRequest, pcm: &[f32])
    -> Result<TailProviderPayload>;
}

/// Existing local Whisper refit behind the W13 provider contract.
#[derive(Debug, Default)]
pub struct InProcessTailProvider;

impl TailProvider for InProcessTailProvider {
    fn provider_id(&self) -> TailProviderId {
        TailProviderId::InProcess
    }

    fn transcribe(
        &self,
        request: &TailProviderRequest,
        pcm: &[f32],
    ) -> Result<TailProviderPayload> {
        request.validate_pcm(pcm)?;
        let started = Instant::now();
        let (speech, _, speech_index) =
            crate::vad::extract_speech_indexed(pcm, request.sample_rate);
        let raw = if speech.is_empty() {
            RawTranscript::default()
        } else {
            super::candle_transcribe_long_with_segments(
                &speech,
                request.sample_rate,
                request.language.as_deref(),
            )?
        };
        let request_range = &request.identity.range;
        let max_compacted = speech.len() as u64;
        let to_sample = |seconds: f32| -> u64 {
            if !seconds.is_finite() || seconds <= 0.0 {
                return 0;
            }
            ((seconds as f64 * request.sample_rate as f64).round() as u64).min(max_compacted)
        };
        let segments = raw
            .segments
            .into_iter()
            .filter_map(|segment| {
                let compacted_start = to_sample(segment.start_ts);
                let compacted_end = to_sample(segment.end_ts).max(compacted_start);
                let (source_start, source_end) = crate::vad::map_compacted_sample_range(
                    &speech_index,
                    compacted_start,
                    compacted_end,
                )?;
                Some(TimedTailSegment {
                    text: segment.text,
                    range: TailSampleRange {
                        session: request_range.session.clone(),
                        capture_epoch: request_range.capture_epoch,
                        sample_start: request.range_end_for(source_start),
                        sample_end: request.range_end_for(source_end),
                    },
                })
            })
            .collect();
        let payload = TailProviderPayload {
            identity: request.identity.clone(),
            text: raw.text,
            segments,
            avg_logprob: raw.avg_logprob,
            compression_ratio: raw.compression_ratio,
            quality_gate_dropped: raw.quality_gate_dropped,
            provider_id: self.provider_id(),
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            evidence: TailProviderEvidence {
                source: TailEvidenceSource::Whisper,
                revision: None,
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: raw.avg_logprob,
            },
        };
        payload.validate()?;
        Ok(payload)
    }
}

impl TailProviderRequest {
    fn range_end_for(&self, relative_sample: u64) -> u64 {
        (self.identity.range.sample_start + relative_sample).min(self.identity.range.sample_end)
    }
}

/// Deterministic fake: an idempotent re-submit of the same request returns the
/// exact same payload, including elapsed time. A different identity is refused.
#[derive(Debug, Clone)]
pub struct FakeTailProvider {
    payload: TailProviderPayload,
}

impl FakeTailProvider {
    pub fn new(payload: TailProviderPayload) -> Result<Self> {
        payload.validate()?;
        if payload.provider_id != TailProviderId::Fake {
            bail!("fake tail provider payload must identify provider_id=fake");
        }
        Ok(Self { payload })
    }
}

impl TailProvider for FakeTailProvider {
    fn provider_id(&self) -> TailProviderId {
        TailProviderId::Fake
    }

    fn transcribe(
        &self,
        request: &TailProviderRequest,
        pcm: &[f32],
    ) -> Result<TailProviderPayload> {
        request.validate_pcm(pcm)?;
        if request.identity != self.payload.identity {
            bail!("fake tail provider request identity mismatch");
        }
        Ok(self.payload.clone())
    }
}

/// Normalized reason why the selected transport yielded to its fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TailProviderFailureKind {
    Unavailable,
    RemoteRequest,
}

impl TailProviderFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::RemoteRequest => "remote_request",
        }
    }
}

/// Content-free proof of which provider actually served one PCM range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailProviderReceipt {
    pub identity: TailRequestIdentity,
    pub requested_provider: TailProviderId,
    pub served_provider: TailProviderId,
    pub fallback_used: bool,
    pub primary_failure: Option<TailProviderFailureKind>,
    pub elapsed_ms: u64,
}

/// Result plus its routing receipt. Keeping the receipt typed lets the real
/// kill-mid-take harness assert fallback without scraping log prose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TailProviderOutcome {
    pub payload: TailProviderPayload,
    pub receipt: TailProviderReceipt,
}

/// Run one selected provider and fall back exactly once. The error itself is
/// deliberately collapsed into a safe category before it reaches telemetry.
pub fn transcribe_with_fallback(
    primary: &dyn TailProvider,
    fallback: &dyn TailProvider,
    primary_failure: TailProviderFailureKind,
    request: &TailProviderRequest,
    pcm: &[f32],
) -> Result<TailProviderOutcome> {
    let started = Instant::now();
    let requested_provider = primary.provider_id();
    match primary.transcribe(request, pcm) {
        Ok(payload) => Ok(TailProviderOutcome {
            receipt: TailProviderReceipt {
                identity: request.identity.clone(),
                requested_provider,
                served_provider: payload.provider_id,
                fallback_used: false,
                primary_failure: None,
                elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            },
            payload,
        }),
        Err(_primary_error) => {
            let payload = fallback.transcribe(request, pcm).with_context(|| {
                format!(
                    "tail provider {} and fallback {} both failed (primary category {})",
                    requested_provider.as_str(),
                    fallback.provider_id().as_str(),
                    primary_failure.as_str()
                )
            })?;
            tracing::debug!(
                requested_provider = requested_provider.as_str(),
                primary_failure = primary_failure.as_str(),
                "tail provider yielded to fallback"
            );
            Ok(TailProviderOutcome {
                receipt: TailProviderReceipt {
                    identity: request.identity.clone(),
                    requested_provider,
                    served_provider: payload.provider_id,
                    fallback_used: true,
                    primary_failure: Some(primary_failure),
                    elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                },
                payload,
            })
        }
    }
}

struct UnavailableTailProvider(TailProviderId);

impl TailProvider for UnavailableTailProvider {
    fn provider_id(&self) -> TailProviderId {
        self.0
    }

    fn transcribe(
        &self,
        _request: &TailProviderRequest,
        _pcm: &[f32],
    ) -> Result<TailProviderPayload> {
        bail!("configured tail provider is unavailable")
    }
}

#[derive(Serialize, Deserialize)]
struct SidecarWireRequest {
    protocol_version: u8,
    token: String,
    request: TailProviderRequest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SidecarWireError {
    Unauthorized,
    Protocol,
    Provider,
}

#[derive(Serialize, Deserialize)]
struct SidecarWireResponse {
    protocol_version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payload: Option<TailProviderPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<SidecarWireError>,
}

/// WebSocket client for one already-running localhost sidecar.
pub struct SidecarTailProvider {
    endpoint: String,
    token: String,
}

impl std::fmt::Debug for SidecarTailProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SidecarTailProvider")
            .field("endpoint", &self.endpoint)
            .field("token", &"[redacted]")
            .finish()
    }
}

impl SidecarTailProvider {
    pub fn new(endpoint: impl Into<String>, token: impl Into<String>) -> Result<Self> {
        let endpoint = endpoint.into();
        sidecar_socket_address(&endpoint)?;
        let token = token.into();
        if token.len() < 32 || token.len() > MAX_TAIL_PROVIDER_ID_BYTES {
            bail!("sidecar token must be 32..={MAX_TAIL_PROVIDER_ID_BYTES} bytes");
        }
        Ok(Self { endpoint, token })
    }
}

impl TailProvider for SidecarTailProvider {
    fn provider_id(&self) -> TailProviderId {
        TailProviderId::Sidecar
    }

    fn transcribe(
        &self,
        request: &TailProviderRequest,
        pcm: &[f32],
    ) -> Result<TailProviderPayload> {
        request.validate_pcm(pcm)?;
        let pcm_bytes = pcm_f32le(pcm)?;
        let address = sidecar_socket_address(&self.endpoint)?;
        let stream = TcpStream::connect_timeout(&address, SIDECAR_CONNECT_TIMEOUT)
            .context("sidecar unavailable")?;
        stream
            .set_read_timeout(Some(SIDECAR_IO_TIMEOUT))
            .context("set sidecar read timeout")?;
        stream
            .set_write_timeout(Some(SIDECAR_IO_TIMEOUT))
            .context("set sidecar write timeout")?;
        let (mut socket, _) = client(self.endpoint.as_str(), stream)
            .map_err(|_| anyhow!("sidecar WebSocket handshake failed"))?;
        let header = SidecarWireRequest {
            protocol_version: SIDECAR_PROTOCOL_VERSION,
            token: self.token.clone(),
            request: request.clone(),
        };
        socket
            .send(Message::Text(serde_json::to_string(&header)?.into()))
            .map_err(|_| anyhow!("sidecar request header send failed"))?;
        socket
            .send(Message::Binary(pcm_bytes.into()))
            .map_err(|_| anyhow!("sidecar PCM send failed"))?;
        let response_message = socket
            .read()
            .map_err(|error| anyhow!("sidecar response read failed: {error}"))?;
        let response_text = response_message
            .into_text()
            .map_err(|_| anyhow!("sidecar response was not JSON text"))?;
        let response: SidecarWireResponse =
            serde_json::from_str(&response_text).context("sidecar response JSON was invalid")?;
        if response.protocol_version != SIDECAR_PROTOCOL_VERSION {
            bail!("sidecar protocol version mismatch");
        }
        if let Some(error) = response.error {
            bail!("sidecar returned normalized error {error:?}");
        }
        let payload = response
            .payload
            .ok_or_else(|| anyhow!("sidecar response omitted payload"))?;
        if payload.identity != request.identity || payload.provider_id != TailProviderId::Sidecar {
            bail!("sidecar response identity/provider mismatch");
        }
        payload.validate()?;
        Ok(payload)
    }
}

fn sidecar_socket_address(endpoint: &str) -> Result<SocketAddr> {
    let url = Url::parse(endpoint).context("invalid sidecar endpoint")?;
    if url.scheme() != "ws" || url.path() != "/tail" || url.query().is_some() {
        bail!("sidecar endpoint must be ws://127.0.0.1:<port>/tail");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("sidecar endpoint has no host"))?;
    let ip: IpAddr = host
        .trim_matches(['[', ']'])
        .parse()
        .map_err(|_| anyhow!("sidecar endpoint must use a numeric loopback host"))?;
    if !ip.is_loopback() {
        bail!("sidecar endpoint must stay on loopback");
    }
    let port = url
        .port()
        .ok_or_else(|| anyhow!("sidecar endpoint has no port"))?;
    Ok(SocketAddr::new(ip, port))
}

fn pcm_f32le(pcm: &[f32]) -> Result<Vec<u8>> {
    let byte_len = pcm
        .len()
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| anyhow!("sidecar PCM byte length overflow"))?;
    if byte_len > MAX_TAIL_PROVIDER_PCM_BYTES {
        bail!("sidecar PCM exceeds bounded request size");
    }
    let mut bytes = Vec::with_capacity(byte_len);
    for sample in pcm {
        if !sample.is_finite() {
            bail!("sidecar PCM contains a non-finite sample");
        }
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(bytes)
}

fn decode_pcm_f32le(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() > MAX_TAIL_PROVIDER_PCM_BYTES || !bytes.len().is_multiple_of(4) {
        bail!("sidecar PCM frame has an invalid bounded length");
    }
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let sample = f32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
            if sample.is_finite() {
                Ok(sample)
            } else {
                bail!("sidecar PCM contains a non-finite sample")
            }
        })
        .collect()
}

/// Serve the sidecar protocol on an explicitly loopback address. This API owns
/// no capture device: its only audio input is the binary PCM WebSocket frame.
pub fn serve_sidecar(
    bind: SocketAddr,
    token: String,
    provider: &dyn TailProvider,
    parent_pid: Option<u32>,
) -> Result<()> {
    if !bind.ip().is_loopback() || token.len() < 32 {
        bail!("sidecar requires loopback bind and a process token");
    }
    let listener = TcpListener::bind(bind).context("bind sidecar loopback listener")?;
    listener
        .set_nonblocking(true)
        .context("set sidecar listener nonblocking")?;
    loop {
        if parent_pid.is_some_and(|pid| !parent_process_alive(pid)) {
            return Ok(());
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = handle_sidecar_connection(stream, &token, provider);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error).context("accept sidecar connection"),
        }
    }
}

fn handle_sidecar_connection(
    stream: TcpStream,
    token: &str,
    provider: &dyn TailProvider,
) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(SIDECAR_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SIDECAR_IO_TIMEOUT))?;
    let mut socket = accept(stream).map_err(|_| anyhow!("sidecar handshake rejected"))?;
    let header_text = socket
        .read()
        .map_err(|error| anyhow!("sidecar header read failed: {error}"))?
        .into_text()
        .map_err(|_| anyhow!("sidecar header was not JSON text"))?;
    let header: SidecarWireRequest = match serde_json::from_str(&header_text) {
        Ok(header) => header,
        Err(_) => return send_sidecar_error(&mut socket, SidecarWireError::Protocol),
    };
    if header.protocol_version != SIDECAR_PROTOCOL_VERSION || header.token != token {
        return send_sidecar_error(&mut socket, SidecarWireError::Unauthorized);
    }
    let pcm_message = socket
        .read()
        .map_err(|_| anyhow!("sidecar PCM read failed"))?;
    if !pcm_message.is_binary() {
        return send_sidecar_error(&mut socket, SidecarWireError::Protocol);
    }
    let pcm = match decode_pcm_f32le(&pcm_message.into_data()) {
        Ok(pcm) => pcm,
        Err(_) => return send_sidecar_error(&mut socket, SidecarWireError::Protocol),
    };
    if header.request.validate_pcm(&pcm).is_err() {
        return send_sidecar_error(&mut socket, SidecarWireError::Protocol);
    }
    let started = Instant::now();
    let mut payload = match provider.transcribe(&header.request, &pcm) {
        Ok(payload) => payload,
        Err(_) => return send_sidecar_error(&mut socket, SidecarWireError::Provider),
    };
    payload.provider_id = TailProviderId::Sidecar;
    payload.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    payload.validate()?;
    let response = SidecarWireResponse {
        protocol_version: SIDECAR_PROTOCOL_VERSION,
        payload: Some(payload),
        error: None,
    };
    socket
        .send(Message::Text(serde_json::to_string(&response)?.into()))
        .map_err(|_| anyhow!("sidecar response send failed"))?;
    Ok(())
}

fn send_sidecar_error(
    socket: &mut tokio_tungstenite::tungstenite::WebSocket<TcpStream>,
    error: SidecarWireError,
) -> Result<()> {
    let response = SidecarWireResponse {
        protocol_version: SIDECAR_PROTOCOL_VERSION,
        payload: None,
        error: Some(error),
    };
    socket
        .send(Message::Text(serde_json::to_string(&response)?.into()))
        .map_err(|_| anyhow!("sidecar error response send failed"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn parent_process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 performs an existence/permission probe and does not
    // deliver a signal to the parent process.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(target_os = "macos"))]
fn parent_process_alive(_pid: u32) -> bool {
    true
}

struct SupervisedSidecar {
    child: Child,
    endpoint: String,
    token: String,
}

impl Drop for SupervisedSidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct SidecarSupervisor {
    process: Mutex<Option<SupervisedSidecar>>,
}

impl SidecarSupervisor {
    fn transcribe(
        &self,
        request: &TailProviderRequest,
        pcm: &[f32],
    ) -> Result<TailProviderPayload> {
        let mut guard = self
            .process
            .lock()
            .map_err(|_| anyhow!("sidecar supervisor lock poisoned"))?;
        let needs_spawn = guard
            .as_mut()
            .map(|process| process.child.try_wait().ok().flatten().is_some())
            .unwrap_or(true);
        if needs_spawn {
            *guard = Some(spawn_sidecar()?);
        }
        let process = guard.as_ref().expect("spawned sidecar");
        let client = SidecarTailProvider::new(&process.endpoint, &process.token)?;
        match client.transcribe(request, pcm) {
            Ok(payload) => Ok(payload),
            Err(error) => {
                *guard = None;
                Err(error)
            }
        }
    }
}

impl TailProvider for SidecarSupervisor {
    fn provider_id(&self) -> TailProviderId {
        TailProviderId::Sidecar
    }

    fn transcribe(
        &self,
        request: &TailProviderRequest,
        pcm: &[f32],
    ) -> Result<TailProviderPayload> {
        self.transcribe(request, pcm)
    }
}

fn spawn_sidecar() -> Result<SupervisedSidecar> {
    let reservation =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("reserve sidecar loopback port")?;
    let address = reservation.local_addr()?;
    drop(reservation);

    let mut token_bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut token_bytes);
    let token = token_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let binary = resolve_sidecar_binary()?;
    let mut child = Command::new(&binary)
        .arg("--bind")
        .arg(address.to_string())
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .env(STT_SIDECAR_TOKEN_ENV, &token)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn sidecar helper {}", binary.display()))?;

    let deadline = Instant::now() + SIDECAR_CONNECT_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            bail!("sidecar helper exited before becoming ready");
        }
        if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("sidecar helper readiness timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(SupervisedSidecar {
        child,
        // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- numeric loopback is enforced by both server and client; TLS adds no trust inside this one-host authenticated channel.
        endpoint: format!("ws://{address}/tail"),
        token,
    })
}

fn resolve_sidecar_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(STT_SIDECAR_BIN_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let executable = std::env::current_exe().context("resolve current executable")?;
    let parent = executable
        .parent()
        .ok_or_else(|| anyhow!("current executable has no parent"))?;
    let sibling = parent.join("codescribe-stt-sidecar");
    if sibling.is_file() {
        return Ok(sibling);
    }
    if parent.file_name().is_some_and(|name| name == "deps") {
        let test_sibling = parent
            .parent()
            .ok_or_else(|| anyhow!("test executable has no target parent"))?
            .join("codescribe-stt-sidecar");
        if test_sibling.is_file() {
            return Ok(test_sibling);
        }
    }
    Ok(PathBuf::from("codescribe-stt-sidecar"))
}

#[derive(Debug)]
pub struct RemoteTailProvider {
    endpoint: String,
    api_key: String,
}

impl RemoteTailProvider {
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Result<Self> {
        let endpoint = endpoint.into();
        validate_remote_endpoint(&endpoint)?;
        let api_key = api_key.into();
        if stt_auth_mode(&endpoint) != SttAuthMode::Unauthenticated && api_key.trim().is_empty() {
            bail!("STT_API_KEY is required for remote tail provider");
        }
        Ok(Self { endpoint, api_key })
    }

    fn from_config() -> Result<Self> {
        let config = crate::config::Config::load();
        let endpoint = config
            .stt_endpoint
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_LOCAL_REMOTE_ENDPOINT.to_string());
        let api_key = config
            .stt_api_key
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_default();
        Self::new(endpoint, api_key)
    }
}

impl TailProvider for RemoteTailProvider {
    fn provider_id(&self) -> TailProviderId {
        TailProviderId::Remote
    }

    fn transcribe(
        &self,
        request: &TailProviderRequest,
        pcm: &[f32],
    ) -> Result<TailProviderPayload> {
        request.validate_pcm(pcm)?;
        let started = Instant::now();
        let wav = pcm16_wav(pcm, request.sample_rate)?;
        let language = request.language.as_deref().unwrap_or("pl");
        let model = std::env::var("WHISPER_MODEL")
            .unwrap_or_else(|_| "mlx-community/whisper-large-v3-mlx".to_string());
        let file = Part::bytes(wav)
            .file_name("tail-window.wav")
            .mime_str("audio/wav")?;
        let form = Form::new()
            .part("file", file)
            .text("model", model.clone())
            .text("language", language.to_string())
            .text("response_format", "verbose_json");
        let http_request = Client::builder()
            .timeout(REMOTE_REQUEST_TIMEOUT)
            .connect_timeout(SIDECAR_CONNECT_TIMEOUT)
            .build()?
            .post(&self.endpoint);
        let http_request = match stt_auth_mode(&self.endpoint) {
            SttAuthMode::Unauthenticated => http_request,
            SttAuthMode::Bearer => http_request.bearer_auth(&self.api_key),
            SttAuthMode::ApiKey => http_request.header("x-api-key", &self.api_key),
        };
        let response = http_request
            .multipart(form)
            .send()
            .context("remote tail request failed")?;
        if !response.status().is_success() {
            bail!("remote tail endpoint returned status {}", response.status());
        }
        let response: RemoteTailResponse = response
            .json()
            .context("remote tail response was not compatible JSON")?;
        let to_absolute = |seconds: f64| -> u64 {
            if !seconds.is_finite() || seconds <= 0.0 {
                return request.identity.range.sample_start;
            }
            request
                .identity
                .range
                .sample_start
                .saturating_add((seconds * request.sample_rate as f64).round() as u64)
                .min(request.identity.range.sample_end)
        };
        let segments = response
            .segments
            .into_iter()
            .map(|segment| TimedTailSegment {
                text: segment.text,
                range: TailSampleRange {
                    session: request.identity.range.session.clone(),
                    capture_epoch: request.identity.range.capture_epoch,
                    sample_start: to_absolute(segment.start),
                    sample_end: to_absolute(segment.end).max(to_absolute(segment.start)),
                },
            })
            .collect();
        let payload = TailProviderPayload {
            identity: request.identity.clone(),
            text: response.text,
            segments,
            avg_logprob: response.avg_logprob,
            compression_ratio: response.compression_ratio,
            quality_gate_dropped: false,
            provider_id: TailProviderId::Remote,
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            evidence: TailProviderEvidence {
                source: TailEvidenceSource::Whisper,
                revision: Some(model),
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: response.avg_logprob,
            },
        };
        payload.validate()?;
        Ok(payload)
    }
}

#[derive(Deserialize)]
struct RemoteTailResponse {
    text: String,
    #[serde(default)]
    segments: Vec<RemoteTailSegment>,
    #[serde(default)]
    avg_logprob: Option<f32>,
    #[serde(default)]
    compression_ratio: Option<f32>,
}

#[derive(Deserialize)]
struct RemoteTailSegment {
    text: String,
    start: f64,
    end: f64,
}

pub(crate) fn validate_remote_endpoint(endpoint: &str) -> Result<()> {
    let url = Url::parse(endpoint).context("invalid remote STT endpoint")?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("remote STT endpoint has no host"))?
        .trim_matches(['[', ']']);
    let loopback = host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("remote STT endpoint requires HTTPS except on loopback");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("remote STT endpoint must not contain credentials");
    }
    Ok(())
}

pub(crate) fn pcm16_wav(pcm: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    if sample_rate == 0 {
        bail!("remote tail sample rate must be non-zero");
    }
    let data_len = pcm
        .len()
        .checked_mul(2)
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or_else(|| anyhow!("remote tail PCM is too large for WAV"))?;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        if !sample.is_finite() {
            bail!("remote tail PCM contains a non-finite sample");
        }
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        wav.extend_from_slice(&value.to_le_bytes());
    }
    Ok(wav)
}

/// Resolve and run the configured provider, emitting only content-free receipt
/// fields. New transports remain opt-in; their failure returns through the
/// in-process provider instead of starving the tail lane.
pub fn transcribe_configured(
    request: &TailProviderRequest,
    pcm: &[f32],
) -> Result<TailProviderPayload> {
    let provider_id = match std::env::var(STT_TAIL_PROVIDER_ENV) {
        Ok(value) => TailProviderId::parse(&value)?,
        Err(std::env::VarError::NotPresent) => TailProviderId::InProcess,
        Err(error) => return Err(error.into()),
    };
    let inprocess = InProcessTailProvider;
    let outcome = match provider_id {
        TailProviderId::InProcess => {
            let started = Instant::now();
            let payload = inprocess.transcribe(request, pcm)?;
            TailProviderOutcome {
                receipt: TailProviderReceipt {
                    identity: request.identity.clone(),
                    requested_provider: TailProviderId::InProcess,
                    served_provider: TailProviderId::InProcess,
                    fallback_used: false,
                    primary_failure: None,
                    elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                },
                payload,
            }
        }
        TailProviderId::Sidecar => {
            static SIDECAR: OnceLock<SidecarSupervisor> = OnceLock::new();
            transcribe_with_fallback(
                SIDECAR.get_or_init(SidecarSupervisor::default),
                &inprocess,
                TailProviderFailureKind::Unavailable,
                request,
                pcm,
            )?
        }
        TailProviderId::Remote => match RemoteTailProvider::from_config() {
            Ok(remote) => transcribe_with_fallback(
                &remote,
                &inprocess,
                TailProviderFailureKind::RemoteRequest,
                request,
                pcm,
            )?,
            Err(_) => transcribe_with_fallback(
                &UnavailableTailProvider(TailProviderId::Remote),
                &inprocess,
                TailProviderFailureKind::RemoteRequest,
                request,
                pcm,
            )?,
        },
        TailProviderId::Fake => unreachable!("fake is injectable, never selected from config"),
    };
    let payload = outcome.payload;
    tracing::info!(
        requested_provider = outcome.receipt.requested_provider.as_str(),
        served_provider = outcome.receipt.served_provider.as_str(),
        fallback_used = outcome.receipt.fallback_used,
        primary_failure = outcome
            .receipt
            .primary_failure
            .map(TailProviderFailureKind::as_str),
        request_id = payload.identity.request_id,
        capture_epoch = payload.identity.range.capture_epoch,
        sample_start = payload.identity.range.sample_start,
        sample_end = payload.identity.range.sample_end,
        segment_count = payload.segments.len(),
        evidence_source = payload.evidence.source.as_str(),
        timing_quality = payload.evidence.timing_quality.as_str(),
        avg_logprob = payload.evidence.avg_logprob,
        provider_elapsed_ms = payload.elapsed_ms,
        routing_elapsed_ms = outcome.receipt.elapsed_ms,
        "tail_provider_receipt"
    );
    Ok(payload)
}

/// Compatibility adapter for current call sites. W13-3A replaces this local
/// identity with the real capture session/epoch and absolute window range.
pub(crate) fn transcribe_legacy_window(
    pcm: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    let request = TailProviderRequest {
        identity: TailRequestIdentity {
            request_id: LEGACY_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
            range: TailSampleRange {
                session: "legacy_tail_patch".to_string(),
                capture_epoch: 0,
                sample_start: 0,
                sample_end: pcm.len() as u64,
            },
        },
        sample_rate,
        language: language.map(str::to_owned),
    };
    transcribe_configured(&request, pcm)?.into_raw_transcript(sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_auth_mode_follows_endpoint_owner() {
        assert_eq!(
            stt_auth_mode("https://api.openai.com/v1/audio/transcriptions"),
            SttAuthMode::Bearer
        );
        assert_eq!(
            stt_auth_mode("https://api.libraxis.cloud/v1/audio/transcriptions"),
            SttAuthMode::Bearer
        );
        assert_eq!(
            stt_auth_mode("http://localhost:8000/v1/audio/transcriptions"),
            SttAuthMode::Unauthenticated
        );
        assert_eq!(
            stt_auth_mode("http://[::1]:8000/v1/audio/transcriptions"),
            SttAuthMode::Unauthenticated
        );
        assert_eq!(
            stt_auth_mode("https://stt.example.test/v1/audio/transcriptions"),
            SttAuthMode::ApiKey
        );
    }

    #[test]
    fn file_probe_endpoint_inverts_known_live_sockets() {
        assert_eq!(
            file_probe_endpoint("https://api.libraxis.cloud/v1/audio/transcriptions"),
            "https://api.libraxis.cloud/v1/audio/transcriptions"
        );
        assert_eq!(
            file_probe_endpoint("wss://api.libraxis.cloud/v1/audio/transcribe"),
            "https://api.libraxis.cloud/v1/audio/transcriptions"
        );
        assert_eq!(
            file_probe_endpoint("ws://127.0.0.1:8000/v1/audio/transcribe"),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            file_probe_endpoint("ws://127.0.0.1:8446/v1/audio/transcribe"),
            "http://127.0.0.1:8444/v1/audio/transcriptions"
        );
        assert_eq!(
            file_probe_endpoint("wss://localhost:8446/v1/audio/transcribe"),
            "https://localhost:8444/v1/audio/transcriptions"
        );
        assert_eq!(
            file_probe_endpoint("wss://stt.example.test/v1/audio/transcribe"),
            "https://stt.example.test/v1/audio/transcriptions"
        );
        assert_eq!(
            file_probe_endpoint("wss://stt.example.test/v1/audio/live"),
            "wss://stt.example.test/v1/audio/live"
        );
    }

    #[test]
    fn w13_tail_provider_contract_typed_payload() {
        let identity = TailRequestIdentity {
            request_id: 17,
            range: TailSampleRange {
                session: "session-typed".to_string(),
                capture_epoch: 3,
                sample_start: 48_000,
                sample_end: 48_320,
            },
        };
        let payload = TailProviderPayload {
            identity: identity.clone(),
            text: "typed result".to_string(),
            segments: vec![TimedTailSegment {
                text: "typed".to_string(),
                range: TailSampleRange {
                    session: "session-typed".to_string(),
                    capture_epoch: 3,
                    sample_start: 48_040,
                    sample_end: 48_200,
                },
            }],
            avg_logprob: Some(-0.21),
            compression_ratio: Some(1.12),
            quality_gate_dropped: false,
            provider_id: TailProviderId::Fake,
            elapsed_ms: 7,
            evidence: TailProviderEvidence {
                source: TailEvidenceSource::Whisper,
                revision: Some("fake-r1".to_string()),
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: Some(-0.21),
            },
        };
        let fake = FakeTailProvider::new(payload.clone()).unwrap();
        let request = TailProviderRequest {
            identity,
            sample_rate: 16_000,
            language: Some("pl-PL".to_string()),
        };
        let pcm = vec![0.0; 320];

        let first = fake.transcribe(&request, &pcm).unwrap();
        let retry = fake.transcribe(&request, &pcm).unwrap();

        assert_eq!(first, payload);
        assert_eq!(retry, first, "same request identity must be idempotent");
        assert_eq!(first.segments[0].range.sample_start, 48_040);
        assert_eq!(first.segments[0].range.sample_end, 48_200);
        assert_eq!(first.avg_logprob, Some(-0.21));
        assert_eq!(first.provider_id, TailProviderId::Fake);
        assert_eq!(first.elapsed_ms, 7);
        first.validate().unwrap();
        assert_eq!(
            TailProviderId::parse("sidecar").unwrap(),
            TailProviderId::Sidecar
        );
        assert_eq!(
            TailProviderId::parse("remote").unwrap(),
            TailProviderId::Remote
        );
        assert_eq!(
            TailProviderId::parse("").unwrap(),
            TailProviderId::InProcess
        );

        let mut different_request = request.clone();
        different_request.identity.request_id += 1;
        assert!(fake.transcribe(&different_request, &pcm).is_err());
    }
}
