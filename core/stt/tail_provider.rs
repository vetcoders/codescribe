//! Typed provider contract for bounded Whisper tail-patch windows.
//!
//! Time on this seam is always an integer PCM sample range. Floating-point
//! seconds exist only in the adapter back to the legacy [`TranscriptSegment`]
//! surface. Hosting and transport belong to W13-2B; this module provides the
//! in-process implementation and a deterministic fake that pin the contract.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::pipeline::contracts::{RawTranscript, TranscriptSegment};

/// Environment key selecting the tail-patch provider.
pub const STT_TAIL_PROVIDER_ENV: &str = "STT_TAIL_PROVIDER";

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

    /// Parse config without arming an implementation. Sidecar and remote are
    /// valid contract values whose hosts land in W13-2B.
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

/// Resolve and run the configured provider, emitting only content-free receipt
/// fields. Sidecar and remote values are parsed now but intentionally remain
/// unarmed until W13-2B supplies their implementations.
pub fn transcribe_configured(
    request: &TailProviderRequest,
    pcm: &[f32],
) -> Result<TailProviderPayload> {
    let provider_id = match std::env::var(STT_TAIL_PROVIDER_ENV) {
        Ok(value) => TailProviderId::parse(&value)?,
        Err(std::env::VarError::NotPresent) => TailProviderId::InProcess,
        Err(error) => return Err(error.into()),
    };
    let payload = match provider_id {
        TailProviderId::InProcess => InProcessTailProvider.transcribe(request, pcm)?,
        TailProviderId::Sidecar | TailProviderId::Remote => {
            bail!(
                "tail provider {} is not armed until W13-2B",
                provider_id.as_str()
            )
        }
        TailProviderId::Fake => unreachable!("fake is injectable, never selected from config"),
    };
    tracing::info!(
        provider = payload.provider_id.as_str(),
        request_id = payload.identity.request_id,
        capture_epoch = payload.identity.range.capture_epoch,
        sample_start = payload.identity.range.sample_start,
        sample_end = payload.identity.range.sample_end,
        segment_count = payload.segments.len(),
        evidence_source = payload.evidence.source.as_str(),
        timing_quality = payload.evidence.timing_quality.as_str(),
        avg_logprob = payload.evidence.avg_logprob,
        elapsed_ms = payload.elapsed_ms,
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
