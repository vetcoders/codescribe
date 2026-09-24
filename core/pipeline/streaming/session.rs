//! Event-based Apple transcription session plus in-memory replay helpers that
//! enter the same single live dispatcher with explicit session/capture identity.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{info, warn};

use crate::asr_session::recorder::{Layer1Decision, RecorderLifecycleEvents};
use crate::audio::streaming_recorder::CaptureTurnIntent;
use crate::config::{Config, RuntimeSettingsSnapshot};
use crate::pipeline::acoustic_ledger::AcousticLedger;
use crate::pipeline::contracts::{
    EngineEvent, EventSink, LayerSummary, SessionConservationReceipt,
};
use crate::stt::tail_patcher::{
    TailPatchConfig, TailPatchOutcome, compute_tail_patch_with_context,
};
#[cfg(test)]
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderId, TailRequestIdentity,
    TailSampleRange, TailTimingQuality, TimedTailSegment,
};
use crate::stt::tail_provider::{TailProviderPayload, TailProviderRequest};

/// Actual execution handles outlive result receivers and ledger accounting.
/// Both live requests and terminal gaps use this session-owned spawn seam.
/// No closure captures the owner itself: the last owner may safely join in Drop.
#[derive(Default)]
pub(crate) struct LocalExecutionOwner {
    control: crate::stt::LocalExecutionControl,
    handles: StdMutex<Vec<std::thread::JoinHandle<()>>>,
}

impl LocalExecutionOwner {
    pub(super) fn check(&self) -> Result<()> {
        self.control.check()
    }

    pub(super) fn begin_drain(&self, budget: std::time::Duration) -> std::time::Instant {
        let deadline = std::time::Instant::now() + budget;
        self.control.limit_until(deadline)
    }

    /// Stop-path text recovery budget. Unlike [`Self::begin_drain`], this
    /// replaces the deadline: recovery runs after the live tail-patch drain
    /// and must not inherit a clock that drain already spent. Cancellation
    /// stays in force.
    pub(super) fn begin_text_recovery(&self, budget: std::time::Duration) -> std::time::Instant {
        self.control
            .replace_deadline(std::time::Instant::now() + budget)
    }

    pub(crate) fn spawn<T, F>(&self, work: F) -> Result<tokio::sync::oneshot::Receiver<Result<T>>>
    where
        T: Send + 'static,
        F: FnOnce(&crate::stt::LocalExecutionControl) -> Result<T> + Send + 'static,
    {
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.control.check()?;
        // Reap finished requests during long captures; never accumulate one
        // native handle per utterance until Stop.
        let mut index = 0;
        while index < handles.len() {
            if handles[index].is_finished() {
                if handles.swap_remove(index).join().is_err() {
                    warn!("local execution worker panicked");
                }
            } else {
                index += 1;
            }
        }
        let control = self.control.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let handle = std::thread::Builder::new()
            .name("local-stt".into())
            .spawn(move || {
                let result = control.check().and_then(|()| work(&control));
                // A native call may return after cancellation. Its label is never
                // a successful completion, even if the provider ignored control.
                let result = control.check().and(result);
                let _ = sender.send(result);
            })?;
        handles.push(handle);
        Ok(receiver)
    }

    pub(crate) async fn close_and_join(&self) {
        self.control.cancel();
        loop {
            let finished = {
                let handles = self
                    .handles
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                handles.iter().all(std::thread::JoinHandle::is_finished)
            };
            if finished {
                break;
            }
            // Retain handles inside self across every await, including caller
            // cancellation. Native calls can exceed the useful-work budget.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.join_retained();
    }

    fn join_retained(&self) {
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for handle in handles.drain(..) {
            if handle.join().is_err() {
                warn!("local execution worker panicked while joining");
            }
        }
    }
}

impl Drop for LocalExecutionOwner {
    fn drop(&mut self) {
        self.control.cancel();
        // Cancellation/panic fallback: blocking is honest here. Dropping a
        // handle would detach native work; no hard release bound is claimed.
        self.join_retained();
    }
}

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
    /// Host-owned Max executor. Presence is transport only, never turn admission.
    pub live_formatting_agent: Option<Arc<dyn crate::ai_formatting::FormattingAgent>>,
    /// The single PCM/evidence/admission owner shared by capture and engines.
    pub acoustic_ledger: Arc<StdMutex<AcousticLedger>>,
    pub sample_rate: u32,
    /// Name of the live input device the recorder actually opened. The
    /// session resolves its `EnergyCalibration` profile by this name; `None`
    /// (offline/buffered harnesses) can never qualify an occurrence.
    pub capture_device_name: Option<String>,
    pub language: Option<String>,
    pub stream_log_path: Option<std::path::PathBuf>,
    /// VAD silence threshold for utterance boundary (None = use default).
    pub utterance_silence_sec: Option<f32>,
    /// How many turns the capture gesture that opened this session owns.
    ///
    /// Frozen per take by the surface that started it. The session reads it
    /// only to decide whether a sealed occurrence may open a *live* paid
    /// formatter slot; acoustic segmentation, ledger qualification, and Layer 1
    /// tail repair are identical for both variants.
    pub capture_turn: CaptureTurnIntent,
    /// Injected, already-authorized Layer 1 refiner decision (C1).
    ///
    /// The pipeline only consumes this — construction, consent, and mode
    /// persistence belong to the settings owner. The decision distinguishes
    /// Apple-only, local exact-span Whisper, and an injected provider.
    pub layer1: Layer1Decision,
    /// Per-recording host lifecycle boundaries. Present only for a live
    /// recorder; buffered/offline helpers have no system observer owner.
    pub lifecycle_events: Option<RecorderLifecycleEvents>,
    /// Recorder stop publishes the complete archive before terminal closure.
    pub terminal_audio: Option<
        std::sync::mpsc::Receiver<Result<super::live_audio_buffer::FinalizedPcmArchive, String>>,
    >,
    /// Acknowledged only after the stop window's L0 events reached the reducer.
    pub last_window_closed: Option<tokio::sync::oneshot::Sender<()>>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailPatchSessionReceipt {
    pub armed: bool,
    pub submitted: u64,
    pub applied: u64,
    pub skipped: u64,
    /// Jobs whose terminal disposition is bounded stop-drain expiry.
    pub timed_out: u64,
    /// Jobs discarded for a non-timeout reason after admission.
    pub abandoned: u64,
    /// Terminal counts above `submitted`. Zero when the buckets fit.
    pub overcount: u64,
    pub drain: TailPatchDrainDisposition,
    /// Conservation loop for this session. Absent until a ledger supplies it.
    pub conservation: SessionConservationReceipt,
}

impl TailPatchSessionReceipt {
    /// Construct a receipt. The caller owns counter provenance; this type names
    /// a mismatch instead of aborting the take.
    ///
    /// A shortfall below `submitted` is added to `abandoned` and the drain
    /// becomes [`TailPatchDrainDisposition::Abandoned`]. An excess is kept in
    /// the caller's buckets, named as `overcount`, and warned.
    pub fn new(
        armed: bool,
        submitted: u64,
        applied: u64,
        skipped: u64,
        timed_out: u64,
        abandoned: u64,
        drain: TailPatchDrainDisposition,
    ) -> Self {
        let accounted = applied
            .saturating_add(skipped)
            .saturating_add(timed_out)
            .saturating_add(abandoned);
        let (abandoned, drain, overcount) = if accounted < submitted {
            (
                abandoned.saturating_add(submitted - accounted),
                TailPatchDrainDisposition::Abandoned,
                0,
            )
        } else if accounted > submitted {
            let overcount = accounted - submitted;
            warn!(
                submitted,
                applied,
                skipped,
                timed_out,
                abandoned,
                overcount,
                "tail-patch terminal buckets over-count submitted jobs"
            );
            (abandoned, drain, overcount)
        } else {
            (abandoned, drain, 0)
        };
        Self {
            armed,
            submitted,
            applied,
            skipped,
            timed_out,
            abandoned,
            overcount,
            drain,
            conservation: SessionConservationReceipt::default(),
        }
    }

    /// Build the production stop receipt. Every job still outstanding after
    /// the worker's real bounded closure loop is classified as timed out.
    /// Any further gap below `submitted` is abandoned by [`Self::new`].
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

    /// Attach the ledger's conservation receipt. Job buckets stay as they are.
    pub fn with_conservation(mut self, conservation: SessionConservationReceipt) -> Self {
        self.conservation = conservation;
        self
    }

    /// An armed lane that submitted no work is a failed runtime witness, not
    /// proof that Layered worked.
    pub fn armed_without_submissions(&self) -> bool {
        self.armed && self.submitted == 0
    }

    /// Whether every submitted job is named by exactly one terminal bucket,
    /// with any excess named by `overcount`.
    pub fn is_reconciled(&self) -> bool {
        self.applied
            .saturating_add(self.skipped)
            .saturating_add(self.timed_out)
            .saturating_add(self.abandoned)
            == self.submitted.saturating_add(self.overcount)
    }

    pub(crate) fn as_event(&self) -> EngineEvent {
        let mut message = format!(
            "armed={} submitted={} applied={} skipped={} timed_out={} abandoned={} overcount={} drain={}",
            self.armed,
            self.submitted,
            self.applied,
            self.skipped,
            self.timed_out,
            self.abandoned,
            self.overcount,
            self.drain.as_token(),
        );
        if self.conservation.emitted {
            message.push(' ');
            message.push_str(&self.conservation.encode_fields());
        }
        EngineEvent::Warning {
            code: TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE.to_string(),
            message,
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
            overcount: fields
                .get("overcount")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            drain: TailPatchDrainDisposition::from_token(fields.get("drain")?)?,
            conservation: SessionConservationReceipt::decode_fields(&fields),
        };
        receipt.is_reconciled().then_some(receipt)
    }
}

/// Re-transcribe a sealed utterance's audio and diff it against the text
/// already committed, producing Layer 1 patch events.
///
/// Runs the Whisper pass on an owned worker so the session loop keeps
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

/// Owned data for one tail job; execution control remains with the session owner.
pub(super) struct TailPatchJobInput {
    pub utterance_id: u64,
    pub committed_text: String,
    pub neighbour_context: String,
    pub audio: Vec<f32>,
    pub request: TailProviderRequest,
    pub config: TailPatchConfig,
}

pub(super) fn compute_tail_patch_job(
    owner: &LocalExecutionOwner,
    input: TailPatchJobInput,
    provider: crate::stt::tail_provider::TailProviderId,
) -> futures_util::future::BoxFuture<'static, Result<TailPatchJobResult>> {
    compute_tail_patch_job_with(owner, input, move |request, pcm, control| {
        crate::stt::tail_provider::transcribe_selected_controlled(provider, request, pcm, control)
    })
}

fn compute_tail_patch_job_with<F>(
    owner: &LocalExecutionOwner,
    input: TailPatchJobInput,
    transcribe: F,
) -> futures_util::future::BoxFuture<'static, Result<TailPatchJobResult>>
where
    F: FnOnce(
            &TailProviderRequest,
            &[f32],
            &crate::stt::LocalExecutionControl,
        ) -> Result<TailProviderPayload>
        + Send
        + 'static,
{
    let TailPatchJobInput {
        utterance_id,
        committed_text,
        neighbour_context,
        audio,
        request,
        config,
    } = input;
    debug_assert_eq!(
        committed_text.trim(),
        committed_text,
        "tail-patch committed_text must be the exact, pre-trimmed UtteranceFinal text \
         (single trim owner: final_text at the emit site)"
    );
    let receiver = owner.spawn(move |control| {
        let payload = transcribe(&request, &audio, control)?;
        control.check()?;
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
    });
    let control = owner.control.clone();
    Box::pin(async move {
        let result = receiver?
            .await
            .map_err(|e| anyhow!("tail patch worker task failed: {e}"))?;
        control.check()?;
        result
    })
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
    conservation: SessionConservationReceipt,
) {
    event_sink.on_event(&EngineEvent::SessionFinalised {
        session_id,
        layer_summary: LayerSummary {
            tail_patch_replacements,
            conservation,
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
pub(super) fn log_tail_patch_session_receipt(receipt: &TailPatchSessionReceipt) {
    if receipt.overcount > 0 {
        warn!(
            armed = receipt.armed,
            submitted = receipt.submitted,
            applied = receipt.applied,
            skipped = receipt.skipped,
            timed_out = receipt.timed_out,
            abandoned = receipt.abandoned,
            overcount = receipt.overcount,
            drain = receipt.drain.as_token(),
            "tail_patch_session_overcount: terminal buckets exceed submitted jobs"
        );
    } else if receipt.timed_out > 0 || receipt.abandoned > 0 {
        warn!(
            armed = receipt.armed,
            submitted = receipt.submitted,
            applied = receipt.applied,
            skipped = receipt.skipped,
            timed_out = receipt.timed_out,
            abandoned = receipt.abandoned,
            overcount = receipt.overcount,
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
            overcount = receipt.overcount,
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
            live_formatting_agent: None,
            acoustic_ledger: Arc::new(StdMutex::new(AcousticLedger::new())),
            sample_rate,
            capture_device_name: None,
            language,
            stream_log_path: None,
            utterance_silence_sec: None,
            // Offline harness: no UI gesture opened this, so it inherits the
            // ordinary hands-free contract rather than inventing a composer take.
            capture_turn: CaptureTurnIntent::HandsFree,
            // Offline replay harness: Layer 1 arming is a live-recording
            // decision owned elsewhere.
            layer1: Layer1Decision::Disarmed,
            lifecycle_events: None,
            terminal_audio: None,
            last_window_closed: None,
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

#[cfg(test)]
/// Unit tests for the session receipt, provider provenance, and the trimmed
/// `final_text` offset baseline every tail-patch range is computed against.
mod session_tests {
    use super::*;

    #[test]
    fn tail_patch_session_receipt_round_trips_through_production_event_shape() {
        let receipt =
            TailPatchSessionReceipt::new(true, 4, 2, 1, 1, 0, TailPatchDrainDisposition::TimedOut);
        assert!(!receipt.armed_without_submissions());
        assert_eq!(
            TailPatchSessionReceipt::from_events(&[receipt.as_event()]),
            Some(receipt)
        );

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
        assert!(completed.is_reconciled());

        let timed_out = TailPatchSessionReceipt::from_stop(true, 3, 1, 0, 2);
        assert_eq!(timed_out.drain, TailPatchDrainDisposition::TimedOut);
        assert_eq!(timed_out.timed_out, 2);
        assert_eq!(timed_out.abandoned, 0);
        assert!(timed_out.is_reconciled());
        assert_eq!(
            TailPatchSessionReceipt::from_events(&[timed_out.as_event()]),
            Some(timed_out)
        );
    }

    /// Stop used to abort the process here: release builds set `panic = "abort"`,
    /// so a shortfall never reached seal or delivery.
    #[test]
    fn stop_receipt_names_unexplained_shortfall_instead_of_aborting() {
        let receipt = TailPatchSessionReceipt::from_stop(true, 3, 1, 1, 0);
        assert_eq!(receipt.applied, 1);
        assert_eq!(receipt.skipped, 1);
        assert_eq!(receipt.timed_out, 0);
        assert_eq!(receipt.abandoned, 1);
        assert_eq!(receipt.overcount, 0);
        assert_eq!(receipt.drain, TailPatchDrainDisposition::Abandoned);
        assert!(receipt.is_reconciled());
        let restored = TailPatchSessionReceipt::from_events(&[receipt.as_event()])
            .expect("a named shortfall stays readable");
        assert_eq!(restored, receipt);
    }

    #[test]
    fn stop_receipt_names_overcount_and_round_trips() {
        let receipt =
            TailPatchSessionReceipt::new(true, 2, 2, 1, 0, 0, TailPatchDrainDisposition::Completed);
        assert_eq!(receipt.overcount, 1);
        assert_eq!(receipt.abandoned, 0);
        assert_eq!(receipt.drain, TailPatchDrainDisposition::Completed);
        assert!(receipt.is_reconciled());
        let EngineEvent::Warning { message, .. } = receipt.as_event() else {
            panic!("receipt must stay a warning event");
        };
        assert!(
            message.contains("overcount=1"),
            "overcount must be a named event field: {message}"
        );
        assert_eq!(
            TailPatchSessionReceipt::from_events(&[receipt.as_event()]),
            Some(receipt)
        );

        let legacy = TailPatchSessionReceipt::from_events(&[EngineEvent::Warning {
            code: TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE.to_string(),
            message: "armed=true submitted=10 applied=4 skipped=3 timed_out=1 abandoned=2 drain=timed_out"
                .to_string(),
        }])
        .expect("events written before overcount= must stay readable");
        assert_eq!(legacy.overcount, 0);
        assert!(legacy.is_reconciled());
        assert_eq!(legacy.drain, TailPatchDrainDisposition::TimedOut);
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
            segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
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
                grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
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

        let owner = LocalExecutionOwner::default();
        let job = compute_tail_patch_job_with(
            &owner,
            TailPatchJobInput {
                utterance_id: 73,
                committed_text: "ala ma kota".to_string(),
                neighbour_context: String::new(),
                audio: vec![0.0; 320],
                request,
                config: TailPatchConfig::default(),
            },
            move |request, pcm, control| {
                control.check()?;
                request.validate_pcm(pcm)?;
                Ok(payload)
            },
        )
        .await
        .expect("typed fake tail job");

        assert_eq!(job.utterance_id, 73);
        assert_eq!(job.payload.identity.request_id, 73);
        assert!(matches!(job.outcome, TailPatchOutcome::NoChange));
        assert_eq!(job.payload.identity.range, range);
        assert_eq!(job.payload.segments[0].range.sample_start, 48_160);
        assert_eq!(job.payload.segments[0].range.sample_end, 48_300);
        assert_eq!(job.payload.evidence, evidence);
        assert_eq!(job.payload.provider_id, TailProviderId::Fake);
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
        emit_session_finalised(
            &collector,
            "session-test".to_string(),
            3,
            SessionConservationReceipt::default(),
        );

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

/// Source-authored W2 falsifiers. These use the production native spawn seam;
/// result-channel disposal is deliberately separate from actual worker exit.
#[cfg(test)]
mod local_execution_tests {
    use super::*;

    #[tokio::test]
    async fn dropped_result_after_accounting_keeps_actual_execution_until_join() {
        let owner = LocalExecutionOwner::default();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let receiver = owner
            .spawn(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok("late label")
            })
            .unwrap();
        entered_rx.await.unwrap();
        // The ledger's accounting may now close and discard its receiver.
        drop(receiver);
        let mut join = Box::pin(owner.close_and_join());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut join)
                .await
                .is_err()
        );
        assert_eq!(owner.handles.lock().unwrap().len(), 1);
        release_tx.send(()).unwrap();
        join.await;
        assert!(owner.handles.lock().unwrap().is_empty());
        assert!(
            owner.spawn(|_| Ok(())).is_err(),
            "closed admission cannot restart"
        );
    }

    #[tokio::test]
    async fn cancelled_native_success_is_error_and_successor_has_independent_control() {
        let owner = LocalExecutionOwner::default();
        let successor = LocalExecutionOwner::default();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let receiver = owner
            .spawn(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok("old-session label")
            })
            .unwrap();
        entered_rx.await.unwrap();
        owner.control.cancel();
        release_tx.send(()).unwrap();
        assert!(receiver.await.unwrap().is_err());
        assert_eq!(
            successor
                .spawn(|_| Ok("new-session label"))
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            "new-session label"
        );
        owner.close_and_join().await;
        successor.close_and_join().await;
    }

    #[tokio::test]
    async fn join_caller_cancellation_retains_handle_for_retry() {
        let owner = LocalExecutionOwner::default();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let receiver = owner
            .spawn(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        entered_rx.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), owner.close_and_join())
                .await
                .is_err()
        );
        assert_eq!(owner.handles.lock().unwrap().len(), 1);
        release_tx.send(()).unwrap();
        owner.close_and_join().await;
        assert!(receiver.await.unwrap().is_err());
        assert!(owner.handles.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn production_tail_job_starts_owned_before_poll_and_rejects_cancelled_completion() {
        tail_job_rejects_late_completion(false).await;
    }

    #[tokio::test]
    async fn production_tail_job_rejects_completion_after_shared_deadline() {
        tail_job_rejects_late_completion(true).await;
    }

    async fn tail_job_rejects_late_completion(expire: bool) {
        let owner = LocalExecutionOwner::default();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: 42,
                range: TailSampleRange {
                    session: "original".into(),
                    capture_epoch: 7,
                    sample_start: 100,
                    sample_end: 104,
                },
            },
            sample_rate: 16_000,
            language: None,
        };
        let job = compute_tail_patch_job_with(
            &owner,
            TailPatchJobInput {
                utterance_id: 42,
                committed_text: "Iwo".into(),
                neighbour_context: String::new(),
                audio: vec![0.25; 4],
                request,
                config: TailPatchConfig::default(),
            },
            move |request, pcm, _| {
                request.validate_pcm(pcm)?;
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(TailProviderPayload {
                    identity: request.identity.clone(),
                    text: "Iwo".into(),
                    segments: vec![TimedTailSegment {
                        grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                        text: "Iwo".into(),
                        range: request.identity.range.clone(),
                    }],
                    avg_logprob: None,
                    compression_ratio: None,
                    provider_id: TailProviderId::Fake,
                    elapsed_ms: 0,
                    evidence: crate::stt::tail_provider::TailProviderEvidence {
                        segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                        source: TailEvidenceSource::Whisper,
                        revision: None,
                        stability: TailEvidenceStability::Final,
                        timing_quality: TailTimingQuality::ExactSampleRange,
                        avg_logprob: None,
                    },
                })
            },
        );
        // No poll of the result future was needed to start and retain work.
        entered_rx.await.unwrap();
        assert_eq!(owner.handles.lock().unwrap().len(), 1);
        if expire {
            let deadline = owner.begin_drain(Duration::ZERO);
            assert_eq!(owner.begin_drain(Duration::from_secs(5)), deadline);
        } else {
            owner.control.cancel();
        }
        release_tx.send(()).unwrap();
        assert!(
            job.await.is_err(),
            "cancelled or expired native success cannot become a tail completion"
        );
        owner.close_and_join().await;
        assert!(owner.handles.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tail_job_rejects_cancelled_or_expired_admission_without_transcribing() {
        for expire in [false, true] {
            let owner = LocalExecutionOwner::default();
            if expire {
                owner.begin_drain(Duration::ZERO);
            } else {
                owner.control.cancel();
            }
            let result = compute_tail_patch_job_with(
                &owner,
                TailPatchJobInput {
                    utterance_id: 91,
                    committed_text: "Iwo".into(),
                    neighbour_context: String::new(),
                    audio: vec![0.25; 4],
                    request: TailProviderRequest {
                        identity: TailRequestIdentity {
                            request_id: 91,
                            range: TailSampleRange {
                                session: "refused-tail-input".into(),
                                capture_epoch: 8,
                                sample_start: 100,
                                sample_end: 104,
                            },
                        },
                        sample_rate: 16_000,
                        language: None,
                    },
                    config: TailPatchConfig::default(),
                },
                |_, _, _| panic!("closed owner must not invoke transcription"),
            )
            .await;
            assert!(result.is_err());
            assert!(owner.handles.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn repeated_drain_cannot_reset_deadline_or_admit_another_gap() {
        let owner = LocalExecutionOwner::default();
        let first = owner.begin_drain(Duration::ZERO);
        assert_eq!(owner.begin_drain(Duration::from_secs(5)), first);
        assert!(owner.spawn(|_| Ok(())).is_err());
    }

    #[test]
    fn text_recovery_budget_replaces_an_expired_live_drain() {
        let owner = LocalExecutionOwner::default();
        let expired = owner.begin_drain(Duration::ZERO);
        assert!(owner.spawn(|_| Ok(())).is_err());
        let recovery = owner.begin_text_recovery(Duration::from_secs(20));
        assert!(recovery > expired);
        let receiver = owner
            .spawn(|_| Ok(7u8))
            .expect("a fresh recovery budget admits work the live drain already refused");
        assert_eq!(receiver.blocking_recv().unwrap().unwrap(), 7);
        assert_eq!(
            owner.begin_drain(Duration::from_secs(60)),
            recovery,
            "begin_drain still cannot move the recovery deadline later"
        );
    }

    #[test]
    fn five_iwo_fixture_session_closes_the_conservation_loop() {
        use crate::pipeline::acoustic_ledger::{
            AcousticEvidence, EnergyCalibration, ObservationIdentity, ObservationProducer,
            OccurrenceIdentity,
        };
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Burst {
            ordinal: usize,
            label: String,
            sample_start: u64,
            sample_end: u64,
            duration_ms: f64,
            energy_integral: f64,
            mean_rms_dbfs: f64,
            peak_dbfs: f64,
            vad_open_sample: u64,
            vad_close_sample: u64,
            evidence_calibration_version: String,
        }
        #[derive(Deserialize)]
        struct Manifest {
            sample_rate: u32,
            minimum_energy_integral: f64,
            minimum_valley_samples: u64,
            expected_occurrences: usize,
            bursts: Vec<Burst>,
        }

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/p0_b_five_iwo_manifest.json");
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(&path).expect("fixture"))
            .expect("five-iwo manifest");
        assert_eq!(manifest.bursts.len(), manifest.expected_occurrences);
        let calibration = EnergyCalibration::new(
            manifest.bursts[0].evidence_calibration_version.clone(),
            manifest.minimum_energy_integral,
            manifest.minimum_valley_samples,
        );
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(manifest.sample_rate);
        for burst in &manifest.bursts {
            let occurrence =
                OccurrenceIdentity::new("p0-b-five-iwo", 1, burst.sample_start, burst.sample_end);
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(),
                duration_ms: burst.duration_ms,
                energy_integral: burst.energy_integral,
                mean_rms_dbfs: burst.mean_rms_dbfs,
                peak_dbfs: burst.peak_dbfs,
                vad_open_sample: Some(burst.vad_open_sample),
                vad_close_sample: Some(burst.vad_close_sample),
                evidence_calibration_version: burst.evidence_calibration_version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            ledger.schedule_frontier(
                occurrence.clone(),
                vec![ObservationProducer::Apple, ObservationProducer::Whisper],
            );
            let apple = ObservationIdentity::new(
                ObservationProducer::Apple,
                burst.ordinal as u64,
                0,
                occurrence.clone(),
            );
            assert!(ledger.admit(&apple, &burst.label).is_insert());
            assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            let whisper = ObservationIdentity::new(
                ObservationProducer::Whisper,
                100 + burst.ordinal as u64,
                0,
                occurrence.clone(),
            );
            assert!(matches!(
                ledger.admit(&whisper, &burst.label),
                crate::pipeline::acoustic_ledger::MutationReceipt::Preserve { .. }
            ));
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
            ledger.seal(&occurrence).expect("burst seals");
        }
        ledger
            .seal_terminal("p0-b-five-iwo", 1)
            .expect("fixture epoch seals");

        let conservation = SessionConservationReceipt::from_ledger(
            &ledger,
            0,
            0,
            0,
            std::collections::BTreeMap::new(),
        );
        let receipt =
            TailPatchSessionReceipt::from_stop(false, 0, 0, 0, 0).with_conservation(conservation);
        assert!(receipt.conservation.emitted);
        assert_eq!(receipt.conservation.windows_admitted, 0);
        assert_eq!(receipt.conservation.windows_coalesced, 0);
        assert_eq!(receipt.conservation.windows_unresolved, 0);
        assert!(
            receipt
                .conservation
                .windows_refused_before_inference
                .is_empty()
        );
        assert_eq!(
            receipt.conservation.first_covered_sample,
            Some(manifest.bursts[0].sample_start)
        );
        assert_eq!(
            receipt.conservation.last_covered_sample,
            manifest.bursts.last().map(|burst| burst.sample_end)
        );
        assert!(receipt.conservation.transcript_seal_timestamp_ms.is_some());
        assert_eq!(receipt.conservation.delivery_timestamp_ms, None);
        assert_eq!(receipt.conservation.observations_admitted, 10);
        assert_eq!(receipt.conservation.observations_delivered, 10);
        assert_eq!(receipt.conservation.observations_unanchored, 0);
        assert!(
            receipt
                .conservation
                .observations_refused_by_reason
                .is_empty()
        );
        assert_eq!(receipt.conservation.energy_lookups_without_voiced_hop, 0);
        assert_eq!(receipt.conservation.residue(), 0);
        assert_eq!(ledger.conservation().residue(), 0);
        assert_eq!(
            ledger.conservation().observations_in,
            ledger.conservation().receipts_out
        );

        let restored = TailPatchSessionReceipt::from_events(&[receipt.as_event()])
            .expect("conservation fields survive the session event");
        assert_eq!(restored.conservation, receipt.conservation);

        let dropped = receipt.conservation.clone().with_receiptless_drop();
        assert_eq!(dropped.residue(), 1);

        let schema = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/fixtures/session_conservation_receipt.schema.json"),
        )
        .expect("conservation schema");
        assert!(schema.contains(crate::pipeline::contracts::SESSION_CONSERVATION_SCHEMA));
        let value = serde_json::to_value(&receipt.conservation).expect("serialize conservation");
        let object = value.as_object().expect("object");
        for key in [
            "windows_admitted",
            "windows_coalesced",
            "windows_unresolved",
            "first_covered_sample",
            "last_covered_sample",
            "transcript_seal_timestamp_ms",
            "delivery_timestamp_ms",
            "observations_admitted",
            "observations_delivered",
            "observations_unanchored",
            "observations_refused_by_reason",
            "windows_refused_before_inference",
            "energy_lookups_without_voiced_hop",
        ] {
            assert!(object.contains_key(key), "receipt missing {key}");
            assert!(schema.contains(key), "schema missing {key}");
        }
    }

    #[test]
    fn window_refused_before_inference_is_not_a_provider_skip() {
        let mut refused = std::collections::BTreeMap::new();
        refused.insert("live_refinement_invalid_identity".to_string(), 1);
        let conservation = SessionConservationReceipt {
            emitted: true,
            windows_refused_before_inference: refused,
            ..SessionConservationReceipt::default()
        };
        let receipt =
            TailPatchSessionReceipt::new(true, 2, 2, 0, 0, 0, TailPatchDrainDisposition::Completed)
                .with_conservation(conservation);
        assert_eq!(receipt.skipped, 0);
        assert!(receipt.is_reconciled());
        assert_eq!(
            receipt.conservation.windows_refused_before_inference["live_refinement_invalid_identity"],
            1
        );
        assert_eq!(receipt.conservation.residue(), 0);
    }
}
