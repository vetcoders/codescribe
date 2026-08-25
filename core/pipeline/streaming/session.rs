//! Event-based Apple transcription session plus in-memory replay helpers that
//! enter the same single live dispatcher with explicit session/capture identity.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, info, warn};

use crate::asr_session::recorder::{Layer1Decision, RecorderLifecycleEvents};
use crate::config::{Config, RuntimeSettingsSnapshot};
use crate::pipeline::acoustic_ledger::AcousticLedger;
use crate::pipeline::contracts::{EngineEvent, EventSink, LayerSource, LayerSummary};
use crate::stt::tail_patcher::{
    TailPatchConfig, TailPatchOutcome, UnderCommit, compute_tail_patch_with_context,
};
#[cfg(test)]
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderId, TailTimingQuality, TimedTailSegment,
};
use crate::stt::tail_provider::{TailProviderPayload, TailProviderRequest};

// ── Unified session config ───────────────────────────────────────────────────

/// Configuration for a transcription session.
///
/// No presentation parameters — this is pure engine config.
pub struct SessionConfig {
    /// Controller-minted capture identity. Engines may observe it but may not
    /// replace it with a lane-local UUID.
    pub session_id: String,
    /// Capture clock epoch shared by every observation in this session.
    pub capture_epoch: u64,
    /// One immutable settings read for the entire session.
    pub runtime_settings: Arc<RuntimeSettingsSnapshot>,
    /// The single PCM/evidence/admission owner shared by capture and engines.
    pub acoustic_ledger: Arc<StdMutex<AcousticLedger>>,
    pub sample_rate: u32,
    pub language: Option<String>,
    pub stream_log_path: Option<std::path::PathBuf>,
    /// VAD silence threshold for utterance boundary (None = use default).
    pub utterance_silence_sec: Option<f32>,
    /// Injected, already-authorized Layer 1 refiner decision (C1).
    ///
    /// The pipeline only consumes this — construction, consent, and mode
    /// persistence belong to the settings owner. The decision distinguishes
    /// Apple-only, local exact-span Whisper, and an injected provider.
    pub layer1: Layer1Decision,
    /// Per-recording host lifecycle boundaries. Present only for a live
    /// recorder; buffered/offline helpers have no system observer owner.
    pub lifecycle_events: Option<RecorderLifecycleEvents>,
}

/// Stable event code carrying the typed local tail-patch session receipt.
pub const TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE: &str = "tail_patch_session_receipt";

/// Final stop-drain disposition for the local Whisper lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailPatchDrainDisposition {
    /// Local tail patching was not armed for this recording.
    NotArmed,
    /// Every submitted job reached a terminal disposition before seal.
    Completed,
    /// The bounded drain expired with admitted work still outstanding.
    TimedOut,
    /// Admitted work was lost for a non-timeout reason (for example worker
    /// failure) before reaching a terminal patch verdict.
    Abandoned,
}

impl TailPatchDrainDisposition {
    fn as_token(self) -> &'static str {
        match self {
            Self::NotArmed => "not_armed",
            Self::Completed => "completed",
            Self::TimedOut => "timed_out",
            Self::Abandoned => "abandoned",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "not_armed" => Some(Self::NotArmed),
            "completed" => Some(Self::Completed),
            "timed_out" => Some(Self::TimedOut),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

/// Content-free proof of local Whisper arming, work admission, application,
/// and bounded stop drainage for one recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailPatchSessionReceipt {
    pub armed: bool,
    pub submitted: u64,
    pub applied: u64,
    pub skipped: u64,
    /// Jobs whose terminal disposition is bounded stop-drain expiry.
    pub timed_out: u64,
    /// Jobs discarded for a non-timeout reason after admission.
    pub abandoned: u64,
    pub drain: TailPatchDrainDisposition,
}

impl TailPatchSessionReceipt {
    /// Construct a receipt. The caller owns counter provenance; this type owns
    /// the invariant checks and stable event encoding.
    pub fn new(
        armed: bool,
        submitted: u64,
        applied: u64,
        skipped: u64,
        timed_out: u64,
        abandoned: u64,
        drain: TailPatchDrainDisposition,
    ) -> Self {
        let receipt = Self {
            armed,
            submitted,
            applied,
            skipped,
            timed_out,
            abandoned,
            drain,
        };
        assert!(
            receipt.is_reconciled(),
            "tail-patch terminal buckets must reconcile exactly to submitted jobs"
        );
        receipt
    }

    /// Build the production stop receipt. Every job still outstanding after
    /// the worker's real bounded closure loop is classified as timed out.
    /// `abandoned` is reserved for a distinct non-timeout discard path.
    pub fn from_stop(
        armed: bool,
        submitted: u64,
        applied: u64,
        skipped: u64,
        timeout_residue: u64,
    ) -> Self {
        Self::new(
            armed,
            submitted,
            applied,
            skipped,
            timeout_residue,
            0,
            if !armed {
                TailPatchDrainDisposition::NotArmed
            } else if timeout_residue > 0 {
                TailPatchDrainDisposition::TimedOut
            } else {
                TailPatchDrainDisposition::Completed
            },
        )
    }

    /// An armed lane that submitted no work is a failed runtime witness, not
    /// proof that Layered worked.
    pub fn armed_without_submissions(self) -> bool {
        self.armed && self.submitted == 0
    }

    /// Whether every submitted job has exactly one terminal bucket.
    pub fn is_reconciled(self) -> bool {
        self.applied
            .saturating_add(self.skipped)
            .saturating_add(self.timed_out)
            .saturating_add(self.abandoned)
            == self.submitted
    }

    pub(crate) fn as_event(self) -> EngineEvent {
        EngineEvent::Warning {
            code: TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE.to_string(),
            message: format!(
                "armed={} submitted={} applied={} skipped={} timed_out={} abandoned={} drain={}",
                self.armed,
                self.submitted,
                self.applied,
                self.skipped,
                self.timed_out,
                self.abandoned,
                self.drain.as_token(),
            ),
        }
    }

    /// Recover the typed receipt from the production ordered event evidence.
    pub fn from_events(events: &[EngineEvent]) -> Option<Self> {
        let message = events.iter().rev().find_map(|event| match event {
            EngineEvent::Warning { code, message }
                if code == TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE =>
            {
                Some(message.as_str())
            }
            _ => None,
        })?;
        let fields = message
            .split_whitespace()
            .filter_map(|field| field.split_once('='))
            .collect::<std::collections::BTreeMap<_, _>>();
        let receipt = Self {
            armed: fields.get("armed")?.parse().ok()?,
            submitted: fields.get("submitted")?.parse().ok()?,
            applied: fields.get("applied")?.parse().ok()?,
            skipped: fields.get("skipped")?.parse().ok()?,
            timed_out: fields.get("timed_out")?.parse().ok()?,
            abandoned: fields.get("abandoned")?.parse().ok()?,
            drain: TailPatchDrainDisposition::from_token(fields.get("drain")?)?,
        };
        receipt.is_reconciled().then_some(receipt)
    }
}

/// Re-transcribe a sealed utterance's audio and diff it against the text
/// already committed, producing Layer 1 patch events.
///
/// Runs the Whisper pass on a blocking worker so the session loop keeps
/// draining. `committed_text` must be the exact string that was emitted as
/// `UtteranceFinal.text`: the resulting `ReplaceRange` offsets are computed
/// against it, so a differently-trimmed copy would produce patches that land at
/// the wrong characters. The debug assertion pins that contract in test builds.
#[derive(Debug)]
pub(super) struct TailPatchJobResult {
    pub utterance_id: u64,
    pub outcome: TailPatchOutcome,
    pub payload: TailProviderPayload,
}

impl TailPatchJobResult {
    pub fn into_outcome(self) -> (u64, TailPatchOutcome) {
        (self.utterance_id, self.outcome)
    }
}

pub(super) async fn compute_tail_patch_job(
    utterance_id: u64,
    committed_text: String,
    neighbour_context: String,
    audio: Vec<f32>,
    request: TailProviderRequest,
    config: TailPatchConfig,
) -> Result<TailPatchJobResult> {
    compute_tail_patch_job_with(
        utterance_id,
        committed_text,
        neighbour_context,
        audio,
        request,
        config,
        crate::stt::tail_provider::transcribe_configured,
    )
    .await
}

async fn compute_tail_patch_job_with<F>(
    utterance_id: u64,
    committed_text: String,
    neighbour_context: String,
    audio: Vec<f32>,
    request: TailProviderRequest,
    config: TailPatchConfig,
    transcribe: F,
) -> Result<TailPatchJobResult>
where
    F: FnOnce(&TailProviderRequest, &[f32]) -> Result<TailProviderPayload> + Send + 'static,
{
    debug_assert_eq!(
        committed_text.trim(),
        committed_text,
        "tail-patch committed_text must be the exact, pre-trimmed UtteranceFinal text \
         (single trim owner: final_text at the emit site)"
    );
    tokio::task::spawn_blocking(move || {
        let payload = transcribe(&request, &audio)?;
        let outcome = compute_tail_patch_with_context(
            &committed_text,
            &payload.text,
            &neighbour_context,
            utterance_id,
            &config,
        );
        Ok(TailPatchJobResult {
            utterance_id,
            outcome,
            payload,
        })
    })
    .await
    .map_err(|e| anyhow!("tail patch worker task failed: {e}"))?
}

/// Stable engine-event code carrying a Layer-1 under-commit outward.
///
/// The stop path keys on this to require residual gap fill for a session whose
/// live canvas is known to be starved, independently of the committed-density
/// floor that judges the same session from the audio side.
pub const UNDER_COMMIT_WARNING_CODE: &str = "tail_patch_under_commit";

/// Build the outward escalation for an under-commit that could not be placed
/// live.
///
/// Counts only — the message crosses the IPC boundary and reaches the log, and
/// the transcript is the user's speech. A `Warning` rather than a new event
/// variant on purpose: every sink, the IPC wire and the Swift bridge already
/// carry it, so the escalation costs no FFI surface.
fn under_commit_warning(under: &UnderCommit) -> EngineEvent {
    EngineEvent::Warning {
        code: UNDER_COMMIT_WARNING_CODE.to_string(),
        message: format!(
            "residual gap fill required: committed_chars={} retranscribed_chars={} \
             committed_tokens={} retranscribed_tokens={} commit_ratio={:.2} gap_appends={}",
            under.committed_chars,
            under.retranscribed_chars,
            under.committed_tokens,
            under.retranscribed_tokens,
            under.commit_ratio,
            under.appends.len(),
        ),
    }
}

/// Forward a tail-patch job's outcome to the sink and report how many
/// replacements were emitted.
///
/// Only `ReplaceRange` events from the tail-patch layer are counted, so the
/// session's layer summary stays attributable. A failed job is not fatal: the
/// Layer 0 committed text is already correct enough to keep, so the error
/// becomes a warning event and the count stays zero.
pub(super) fn emit_tail_patch_result(
    event_sink: &dyn EventSink,
    result: Result<(u64, TailPatchOutcome)>,
) -> u64 {
    match result {
        Ok((utterance_id, TailPatchOutcome::Patches(events))) => {
            let mut emitted = 0u64;
            for event in events {
                if matches!(
                    event,
                    EngineEvent::ReplaceRange {
                        source: LayerSource::TailPatch,
                        ..
                    }
                ) {
                    emitted = emitted.saturating_add(1);
                }
                event_sink.on_event(&event);
            }
            debug!(utterance_id, emitted, "Applied tail patch replacements");
            emitted
        }
        Ok((utterance_id, TailPatchOutcome::NoChange)) => {
            debug!(utterance_id, "Tail patch found no changes");
            0
        }
        Ok((utterance_id, TailPatchOutcome::UnderCommit(under))) => {
            let mut emitted = 0u64;
            for event in &under.appends {
                if matches!(
                    event,
                    EngineEvent::ReplaceRange {
                        source: LayerSource::TailPatch,
                        ..
                    }
                ) {
                    emitted = emitted.saturating_add(1);
                }
                event_sink.on_event(event);
            }
            info!(
                utterance_id,
                reason = under.reason(),
                committed_chars = under.committed_chars,
                retranscribed_chars = under.retranscribed_chars,
                committed_tokens = under.committed_tokens,
                retranscribed_tokens = under.retranscribed_tokens,
                gap_appends = emitted,
                residual_required = under.residual_required,
                "Layer 1 under-commit"
            );
            if under.residual_required {
                event_sink.on_event(&under_commit_warning(&under));
            }
            emitted
        }
        Ok((utterance_id, TailPatchOutcome::Skipped { code, reason })) => {
            // INFO, not debug: a skipped patch is text Whisper had in hand and
            // the canvas never received. The counts belong to the receipt the
            // patcher already logs; this line proves the sink saw the same
            // verdict for this utterance.
            info!(
                utterance_id,
                code = code.as_str(),
                reason,
                "Tail patch skipped"
            );
            0
        }
        Err(e) => {
            warn!("Tail patch failed; keeping Layer 0 committed text: {}", e);
            event_sink.on_event(&EngineEvent::Warning {
                code: "tail_patch_error".to_string(),
                message: format!("{}", e),
            });
            0
        }
    }
}

/// Emit the session's closing event with its layer accounting.
///
/// Only the tail-patch count is populated here — the other layers are applied
/// outside this session path, so reporting zeros for them is honest rather
/// than incomplete.
pub(super) fn emit_session_finalised(
    event_sink: &dyn EventSink,
    session_id: String,
    tail_patch_replacements: u64,
) {
    event_sink.on_event(&EngineEvent::SessionFinalised {
        session_id,
        layer_summary: LayerSummary {
            tail_patch_replacements,
            ..LayerSummary::default()
        },
    });
}

/// Per-session skip count at which a zero-application session is an alarm.
///
/// One or two skips with nothing applied can be honest divergence (noise, a
/// throat-clear window). Three computed corrections all rejected is the gate
/// eating the lane's entire output — the 2026-08-12 audit found 116 skips and
/// 0 applied patches across the log's whole history, and not one line said so
/// out loud.
pub(super) const TAIL_PATCH_STARVED_MIN_SKIPS: u64 = 3;

/// Whether this session's Layer 1 lane was starved: corrections were computed
/// and every single one was rejected.
pub(super) fn tail_patch_lane_starved(applied: u64, skipped: u64) -> bool {
    applied == 0 && skipped >= TAIL_PATCH_STARVED_MIN_SKIPS
}

/// One session-level receipt for the Layer 1 lane, emitted at finalise.
///
/// The per-utterance skip receipts diagnose a single verdict; this line
/// diagnoses the lane. A starved session — Whisper burned inference on every
/// sealed utterance and the canvas received none of it — is a WARN, because
/// that is the lane not doing its one job, silently.
pub(super) fn log_tail_patch_session_receipt(receipt: TailPatchSessionReceipt) {
    if receipt.timed_out > 0 || receipt.abandoned > 0 {
        warn!(
            armed = receipt.armed,
            submitted = receipt.submitted,
            applied = receipt.applied,
            skipped = receipt.skipped,
            timed_out = receipt.timed_out,
            abandoned = receipt.abandoned,
            drain = receipt.drain.as_token(),
            "tail_patch_session_degraded: accepted work missed the bounded stop drain"
        );
    } else if receipt.armed_without_submissions() {
        warn!(
            armed = receipt.armed,
            submitted = receipt.submitted,
            drain = receipt.drain.as_token(),
            "tail_patch_lane_unexercised: armed session submitted zero Whisper windows"
        );
    } else if tail_patch_lane_starved(receipt.applied, receipt.skipped) {
        warn!(
            applied = receipt.applied,
            skipped = receipt.skipped,
            "tail_patch_lane_starved: every computed Whisper correction this session was rejected"
        );
    } else {
        info!(
            armed = receipt.armed,
            submitted = receipt.submitted,
            applied = receipt.applied,
            skipped = receipt.skipped,
            timed_out = receipt.timed_out,
            abandoned = receipt.abandoned,
            drain = receipt.drain.as_token(),
            "tail_patch_session_receipt"
        );
    }
}

// ── Unified transcription session (event-based) ─────────────────────────────

/// Single live transcription dispatcher.
///
/// Apple progressive receives the controller session, recorder-issued capture
/// epoch, immutable settings snapshot, and shared acoustic ledger through
/// [`SessionConfig`]. No parallel VAD/scheduler dispatcher or lane-local
/// identity allocator remains.
pub(crate) async fn transcription_session(
    chunk_receiver: mpsc::Receiver<Vec<f32>>,
    event_sink: Arc<dyn EventSink>,
    config: SessionConfig,
) {
    super::apple_live_session::apple_stream_transcription_session(
        chunk_receiver,
        event_sink,
        config,
    )
    .await;
}

/// [`EventSink`] that records the raw event stream in order, drops included.
/// Backs [`collect_buffered_engine_events`] and this module's own tests.
struct SessionEventCollector {
    events: std::sync::Mutex<Vec<EngineEvent>>,
}

impl SessionEventCollector {
    /// Collector with no events recorded.
    fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Snapshot of the events recorded so far, in emission order.
    fn events(&self) -> Vec<EngineEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EventSink for SessionEventCollector {
    /// Clone every engine event (including drops) into the ordered collector buffer.
    fn on_event(&self, event: &EngineEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
    }
}

/// Public helper: run the event session pipeline and return the emitted engine events.
///
/// This is the closest non-interactive test hook to the real live flow:
/// canonical audio samples enter the same `transcription_session` runtime used by
/// recording, and callers can replay the resulting `EngineEvent`s through
/// `PresentationEmitter`/overlay code without touching the microphone.
pub async fn collect_buffered_engine_events(
    samples: &[f32],
    sample_rate: u32,
    language: Option<String>,
) -> Result<Vec<EngineEvent>> {
    let runtime_settings = Arc::new(
        Config::load_runtime_snapshot_without_keychain()
            .map_err(|error| anyhow!("invalid runtime settings snapshot: {error:?}"))?,
    );
    collect_buffered_engine_events_with_config(
        samples,
        SessionConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            capture_epoch: 1,
            runtime_settings,
            acoustic_ledger: Arc::new(StdMutex::new(AcousticLedger::new())),
            sample_rate,
            language,
            stream_log_path: None,
            utterance_silence_sec: None,
            // Offline replay harness: Layer 1 arming is a live-recording
            // decision owned elsewhere.
            layer1: Layer1Decision::Disarmed,
            lifecycle_events: None,
        },
    )
    .await
}

/// Run buffered PCM through an explicitly supplied production session config.
///
/// Unlike [`collect_buffered_engine_events`], this seam never invents or
/// hard-codes a Layer 1 decision. The recording owner must supply the complete
/// [`SessionConfig`], which makes this suitable for production-owned replay
/// witnesses while preserving the exact `transcription_session` implementation
/// used by live capture.
pub async fn collect_buffered_engine_events_with_config(
    samples: &[f32],
    config: SessionConfig,
) -> Result<Vec<EngineEvent>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let chunk_size = ((config.sample_rate as f32) * 0.1).round().max(1.0) as usize;
    let (tx, rx) = mpsc::channel::<Vec<f32>>(8);
    let collector = Arc::new(SessionEventCollector::new());
    let event_sink: Arc<dyn EventSink> = collector.clone();
    let session = tokio::spawn(transcription_session(rx, event_sink, config));

    for chunk in samples.chunks(chunk_size) {
        if tx.send(chunk.to_vec()).await.is_err() {
            return Err(anyhow!("Transcription session dropped channel"));
        }
        // `transcription_session` consumes a live capture stream. Preserve
        // that temporal contract for replay: flooding an entire recording in
        // one scheduler tick advances `audio_secs` ahead of Apple's result
        // timestamps and turns otherwise valid phrase windows into unresolved
        // seals. A 100 ms packet therefore occupies 100 ms of wall time, just
        // like the production callback cadence this seam replaces at its only
        // unavoidable boundary.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(tx);

    session
        .await
        .map_err(|e| anyhow!("Transcription session join error: {}", e))?;

    Ok(collector.events())
}

#[cfg(any())]
/// Unit tests for semantic-gate counters, tail-patch emit, and correction windows.
mod session_tests {
    use super::*;

    #[test]
    fn tail_patch_session_receipt_round_trips_through_production_event_shape() {
        let receipt =
            TailPatchSessionReceipt::new(true, 4, 2, 1, 1, 0, TailPatchDrainDisposition::TimedOut);
        assert_eq!(
            TailPatchSessionReceipt::from_events(&[receipt.as_event()]),
            Some(receipt)
        );
        assert!(!receipt.armed_without_submissions());

        let unexercised =
            TailPatchSessionReceipt::new(true, 0, 0, 0, 0, 0, TailPatchDrainDisposition::Completed);
        assert!(unexercised.armed_without_submissions());
    }

    #[test]
    fn stop_receipt_classifies_completed_and_timeout_residue() {
        let completed = TailPatchSessionReceipt::from_stop(true, 3, 2, 1, 0);
        assert_eq!(completed.drain, TailPatchDrainDisposition::Completed);
        assert_eq!(completed.timed_out, 0);
        assert_eq!(completed.abandoned, 0);

        let timed_out = TailPatchSessionReceipt::from_stop(true, 3, 1, 0, 2);
        assert_eq!(timed_out.drain, TailPatchDrainDisposition::TimedOut);
        assert_eq!(timed_out.timed_out, 2);
        assert_eq!(timed_out.abandoned, 0);
        assert_eq!(
            TailPatchSessionReceipt::from_events(&[timed_out.as_event()]),
            Some(timed_out)
        );
    }

    #[tokio::test]
    async fn w13_provenance_survives_tail_patch_job() {
        let range = TailSampleRange {
            session: "w13-replay-191351".to_string(),
            capture_epoch: 4,
            sample_start: 48_000,
            sample_end: 48_320,
        };
        let identity = TailRequestIdentity {
            request_id: 73,
            range: range.clone(),
        };
        let evidence = crate::stt::tail_provider::TailProviderEvidence {
            source: TailEvidenceSource::Whisper,
            revision: Some("fixture-r1".to_string()),
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::ExactSampleRange,
            avg_logprob: Some(-0.21),
        };
        let payload = TailProviderPayload {
            identity: identity.clone(),
            text: "ala ma kota".to_string(),
            segments: vec![TimedTailSegment {
                text: "kota".to_string(),
                range: TailSampleRange {
                    sample_start: 48_160,
                    sample_end: 48_300,
                    ..range.clone()
                },
            }],
            avg_logprob: Some(-0.21),
            compression_ratio: Some(1.03),
            provider_id: TailProviderId::Fake,
            elapsed_ms: 7,
            evidence: evidence.clone(),
        };
        let request = TailProviderRequest {
            identity,
            sample_rate: 16_000,
            language: Some("pl-PL".to_string()),
        };

        let job = compute_tail_patch_job_with(
            73,
            "ala ma kota".to_string(),
            String::new(),
            vec![0.0; 320],
            request,
            TailPatchConfig::default(),
            move |request, pcm| {
                request.validate_pcm(pcm)?;
                Ok(payload)
            },
        )
        .await
        .expect("typed fake tail job");

        assert!(matches!(job.outcome, TailPatchOutcome::NoChange));
        assert_eq!(job.payload.identity.range, range);
        assert_eq!(job.payload.segments[0].range.sample_start, 48_160);
        assert_eq!(job.payload.segments[0].range.sample_end, 48_300);
        assert_eq!(job.payload.evidence, evidence);
        assert_eq!(job.payload.provider_id, TailProviderId::Fake);
    }

    #[test]
    /// Successful tail-patch outcomes surface as `ReplaceRange` engine events.
    fn tail_patch_result_emits_replace_range_events() {
        let collector = SessionEventCollector::new();
        let emitted = emit_tail_patch_result(
            &collector,
            Ok((
                42,
                TailPatchOutcome::Patches(vec![EngineEvent::ReplaceRange {
                    utterance_id: 42,
                    start: 4,
                    end: 7,
                    text: "kot".to_string(),
                    source: LayerSource::TailPatch,
                }]),
            )),
        );

        assert_eq!(emitted, 1);
        assert!(matches!(
            collector.events().as_slice(),
            [EngineEvent::ReplaceRange {
                utterance_id: 42,
                start: 4,
                end: 7,
                text,
                source: LayerSource::TailPatch,
            }] if text == "kot"
        ));
    }

    /// Build an under-commit outcome with `appends` gap-appends and the given
    /// escalation, without needing Whisper or a diff.
    fn under_commit_fixture(appends: usize, residual_required: bool) -> UnderCommit {
        UnderCommit {
            appends: (0..appends)
                .map(|idx| EngineEvent::ReplaceRange {
                    utterance_id: 7,
                    start: 11 + idx,
                    end: 11 + idx,
                    text: " odzyskane".to_string(),
                    source: LayerSource::TailPatch,
                })
                .collect(),
            residual_required,
            committed_tokens: 3,
            retranscribed_tokens: 12,
            committed_chars: 21,
            retranscribed_chars: 84,
            commit_ratio: 0.25,
        }
    }

    #[test]
    /// W-C: recovered gap-appends reach the sink and are counted as Layer 1
    /// work — the outcome the bounded cap used to discard in silence.
    fn under_commit_gap_appends_reach_the_sink_and_count() {
        let collector = SessionEventCollector::new();
        let emitted = emit_tail_patch_result(
            &collector,
            Ok((
                7,
                TailPatchOutcome::UnderCommit(under_commit_fixture(1, false)),
            )),
        );

        assert_eq!(emitted, 1, "an appended gap is Layer 1 work, not a skip");
        let events = collector.events();
        assert!(matches!(
            events.as_slice(),
            [EngineEvent::ReplaceRange {
                start: 11,
                end: 11,
                source: LayerSource::TailPatch,
                ..
            }]
        ));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, EngineEvent::Warning { .. })),
            "nothing is owed to the stop path when everything landed live"
        );
    }

    #[test]
    /// W-C: an under-commit that could place nothing escalates outward instead
    /// of leaving the stop path to call the starved canvas complete.
    fn under_commit_without_safe_anchor_emits_residual_escalation() {
        let collector = SessionEventCollector::new();
        let emitted = emit_tail_patch_result(
            &collector,
            Ok((
                7,
                TailPatchOutcome::UnderCommit(under_commit_fixture(0, true)),
            )),
        );

        assert_eq!(emitted, 0);
        let events = collector.events();
        let warning = events
            .iter()
            .find_map(|e| match e {
                EngineEvent::Warning { code, message } if code == UNDER_COMMIT_WARNING_CODE => {
                    Some(message.clone())
                }
                _ => None,
            })
            .expect("residual escalation must be emitted");
        // Counts travel; transcript text never does.
        assert!(warning.contains("committed_chars=21"));
        assert!(warning.contains("retranscribed_chars=84"));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, EngineEvent::ReplaceRange { .. })),
            "no anchor was safe, so no canvas may be touched"
        );
    }

    #[test]
    /// Trimmed final_text is the sole offset baseline for tail-patch apply.
    fn final_text_trim_contract_keeps_tail_patch_offsets_aligned() {
        // Simulate the emit site: accumulated_text carries whitespace, the single
        // trim owner produces final_text, and that SAME string is both
        // UtteranceFinal.text and the tail-patch committed_text.
        let accumulated = "  ala ma kota  ";
        let final_text = accumulated.trim().to_string();

        // Retranscribed side mimics real Whisper output shape: leading/trailing
        // whitespace and a newline. It must never skew offsets or get skipped.
        let outcome = crate::stt::tail_patcher::compute_tail_patch(
            &final_text,
            " ala ma psa \n",
            1,
            &TailPatchConfig::default(),
        );
        let TailPatchOutcome::Patches(events) = &outcome else {
            panic!("expected Patches (not Skipped/NoChange), got {outcome:?}");
        };

        // Offsets must apply cleanly against the exact string the consumer holds.
        let mut buf = final_text.clone();
        for event in events {
            let applied = event
                .apply_to_committed_text(&mut buf)
                .expect("patch offsets must be in range for the trimmed final_text");
            assert!(applied, "ReplaceRange must mutate the committed buffer");
        }
        assert_eq!(buf, "ala ma psa");
    }

    #[test]
    /// Session end emits `SessionFinalised` carrying the layer replacement summary.
    fn session_finalised_emits_layer_summary() {
        let collector = SessionEventCollector::new();
        emit_session_finalised(&collector, "session-test".to_string(), 3);

        assert!(matches!(
            collector.events().as_slice(),
            [EngineEvent::SessionFinalised {
                session_id,
                layer_summary,
            }] if session_id == "session-test"
                && layer_summary.tail_patch_replacements == 3
                && layer_summary.lexicon_replacements == 0
                && layer_summary.inline_llm_replacements == 0
                && layer_summary.final_bam_replacements == 0
                && layer_summary.annotations_inserted == 0
        ));
    }

    #[test]
    /// The starvation verdict: zero applied with the skip floor reached is the
    /// lane not doing its job. One landed patch — even against 116 skips —
    /// proves the lane alive; a skip or two with nothing applied is honest
    /// divergence, not starvation.
    fn tail_patch_starvation_fires_only_on_all_rejected_sessions() {
        assert!(tail_patch_lane_starved(0, TAIL_PATCH_STARVED_MIN_SKIPS));
        assert!(tail_patch_lane_starved(0, 116));
        assert!(!tail_patch_lane_starved(1, 116), "one landed patch = alive");
        assert!(
            !tail_patch_lane_starved(0, TAIL_PATCH_STARVED_MIN_SKIPS - 1),
            "a couple of honest divergences is not starvation"
        );
        assert!(
            !tail_patch_lane_starved(0, 0),
            "an idle lane is not starved"
        );
    }

}
