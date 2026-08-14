//! Apple progressive live session — system-dictation shape.
//!
//! Bypasses Silero-VAD + per-window Whisper scheduler for the Apple live path.
//! One long-lived SFSpeech stream maps:
//! - `partial` → `EngineEvent::Preview` (RAW — previews are not canvas yet)
//! - phrase `final` → `EngineEvent::UtteranceFinal` (multi-seal freezed+append)
//! - open partial on stop → sealed as a last final when non-empty
//!
//! Every seal runs the shared `StreamPostProcessor::process_utterance` pass
//! (lexicon + cleanup, no semantic gate) BEFORE the text becomes committed
//! canvas — the daily-driver path must satisfy AGENTS.md item 3 ("lexicon
//! corrections applied on the fly"). Correcting after commit would be a
//! post-commit rewrite, which the append-only doctrine forbids.
//!
//! Whisper is never the live engine here. Under
//! `CODESCRIBE_LAYERED_TRANSCRIPTION=phase1+` it runs as Layer 1 gap-fill
//! (W2-A): each sealed utterance resolves to its retained PCM window and is
//! re-transcribed off this path, emitting bounded
//! `ReplaceRange { source: TailPatch }` events — AGENTS.md (THE ONE RULE):
//! filling canvas gaps on the go, never a stop-time full-text authority.
//! Outside that flag Whisper stays the file final-pass / emergency fill
//! (controller stop path). Escape hatch:
//! `CODESCRIBE_APPLE_STT_LIVE_MODE=wav` restores the legacy VAD+scheduler path.
//!
//! The bridge global lock + child process live on a **dedicated OS thread**
//! (MutexGuard is `!Send`); the async session only shuttles PCM in and
//! `EngineEvent`s out.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesOrdered;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::asr_session::recorder::{
    LAYER1_DEGRADED_WARNING_CODE, Layer1DegradeReason, RecorderLayer1Lane,
    apply_recorder_lifecycle_event,
};
use crate::asr_session::{SessionId as Layer1SessionId, SessionInput as Layer1SessionInput};
use crate::audio::capture_receipt::{
    CaptureLevelAccumulator, CapturePathMeta, emit_capture_level_receipt,
};
use crate::pipeline::contracts::{DropKind, EngineEvent, EventSink, TranscriptSegment};
use crate::pipeline::stream_postprocess::StreamPostProcessor;
use crate::stt::apple_stt::{LiveStreamEvent, LiveStreamSession};
use crate::stt::tail_patcher::{SkipReasonCode, TailPatchConfig, TailPatchOutcome};
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderPayload,
    TailProviderRequest, TailRequestIdentity, TailSampleRange, TailTimingQuality, TimedTailSegment,
};

use super::live_audio_buffer::{DEFAULT_RETENTION_SECS, LiveAudioBuffer, ResolvedAudioWindow};
use super::progressive_seal::{ProgressiveSealMachine, SealTick, seal_span_text};
#[cfg(test)]
use super::session::emit_tail_patch_result;
use super::session::{
    SessionConfig, TailPatchJobResult, UNDER_COMMIT_WARNING_CODE, compute_tail_patch_job,
    emit_session_finalised, log_tail_patch_session_receipt, tail_patch_enabled,
};
use super::silero_fusion::{
    FusionContextMode, FusionWord, SileroIngress, bound_context_range, conservative_fuse,
    fusion_receipt, lane_enabled, slice_apple_words,
};
use super::stream_log::append_to_stream_log;

/// How many sealed utterances may wait for Layer 1 before the seal path starts
/// dropping requests.
///
/// The seal path runs on the worker thread that also forwards PCM into the
/// SFSpeech bridge, so it must never block on this queue — a stalled worker is
/// a stalled capture. A bounded queue plus a counted drop is the honest
/// backpressure shape: Whisper falling behind costs patches, never audio.
const TAIL_PATCH_QUEUE_CAP: usize = 8;

/// How long the end-of-session closure loop waits for one outstanding Layer 1
/// job to report back.
///
/// This sits directly on the stop path, in front of an operator watching the
/// overlay, so it is a product budget rather than an engineering safety net.
/// Every job it waits for was queued during capture against a model that is
/// already warm, and the observed windows close well under a second; the cap is
/// here for a genuinely wedged job, not for normal completion. It was 30s until
/// 2026-08-12, when a stop that owed nothing at all still paid the full 30s
/// because the loop was waiting on the wrong condition.
const TAIL_PATCH_CLOSURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Content-free marker emitted when an Apple final callback contained segment
/// time already committed by an earlier callback. The overlapping portion is
/// removed before a new utterance id can be allocated.
pub const APPLE_FINAL_OVERLAP_WARNING_CODE: &str = "apple_final_window_overlap_normalized";

/// One sealed utterance handed from the worker thread to the async Layer 1 lane.
struct TailPatchRequest {
    utterance_id: u64,
    /// Byte-identical to the emitted `UtteranceFinal.text` — the string every
    /// `ReplaceRange` char offset is computed against.
    committed_text: String,
    /// PCM behind exactly this utterance: `[previous seal end, end_ts)`.
    audio: Vec<f32>,
    /// Exact capture range behind `audio`; this is the window-start authority.
    provider_request: TailProviderRequest,
    covered_through_secs: f32,
}

/// Whisper closure returned to the worker that owns Apple + seal state.
struct TailPatchCompletion {
    utterance_id: u64,
    covered_through_secs: f32,
    outcome: TailPatchOutcome,
    payload: Option<TailProviderPayload>,
}

/// Async Layer 1 lane for the Apple progressive path.
///
/// Owns the in-flight Whisper gap-fill job and the replacement count that
/// `SessionFinalised.layer_summary` reports. Jobs are boxed so the lane can be
/// driven by a stub future in tests without a model on disk.
struct AppleTailPatchLane {
    jobs: FuturesOrdered<BoxFuture<'static, Result<TailPatchJobResult>>>,
    language: Option<String>,
    config: TailPatchConfig,
    replacements: u64,
    /// Jobs whose entire output was rejected (Skipped or failed). Feeds the
    /// session-level starvation receipt — the 116-skips/0-applied class of
    /// silent lane death must be one WARN, not a grep across log history.
    skipped: u64,
}

impl AppleTailPatchLane {
    /// Open an empty lane. `TailPatchConfig::from_env` is read once here so the
    /// whole session judges every patch against the same thresholds, even if the
    /// env flips mid-hold.
    fn new(_sample_rate: u32, language: Option<String>) -> Self {
        Self {
            jobs: FuturesOrdered::new(),
            language,
            // F2: thresholds stay exactly where the shared primitive puts them.
            config: TailPatchConfig::from_env(),
            replacements: 0,
            skipped: 0,
        }
    }

    /// Turn a sealed utterance into a Whisper gap-fill job and queue it. The job
    /// is only constructed — inference happens inside it on `spawn_blocking`, so
    /// this call never sits on the event-drain path.
    fn push_request(&mut self, mut req: TailPatchRequest) {
        req.provider_request.language = self.language.clone();
        let job = compute_tail_patch_job(
            req.utterance_id,
            req.committed_text,
            req.audio,
            req.provider_request,
            self.config,
        );
        self.push_job(Box::pin(job));
    }

    /// Queue an already-built job. Boxed and separate from `push_request` so
    /// tests can drive the lane with a stub future, with no model on disk.
    fn push_job(&mut self, job: BoxFuture<'static, Result<TailPatchJobResult>>) {
        self.jobs.push_back(job);
    }

    /// Await the next finished job. `FuturesOrdered` (not `Unordered`) is the
    /// point: patches must reach the sink in seal order, or a later utterance's
    /// `ReplaceRange` could land before an earlier one's.
    async fn next(&mut self) -> Option<Result<TailPatchJobResult>> {
        self.jobs.next().await
    }

    /// Emit a finished job's patch and fold its replacement count into the
    /// session total. A skipped or failed patch contributes zero — only text
    /// that actually reached the canvas is counted.
    #[cfg(test)]
    fn complete(&mut self, event_sink: &dyn EventSink, result: Result<TailPatchJobResult>) {
        if matches!(&result, Ok(job) if matches!(job.outcome, TailPatchOutcome::Skipped { .. }))
            || result.is_err()
        {
            self.skipped = self.skipped.saturating_add(1);
        }
        let result = result.map(TailPatchJobResult::into_outcome);
        self.replacements = self
            .replacements
            .saturating_add(emit_tail_patch_result(event_sink, result));
    }

    /// Convert a finished job into the closure message consumed by the
    /// progressive seal owner. Patch events are emitted by that owner only
    /// after `UtteranceFinal`, preserving event order.
    fn finish_for_worker(
        &mut self,
        request_id: u64,
        req_end_secs: f32,
        result: Result<TailPatchJobResult>,
    ) -> TailPatchCompletion {
        match result {
            Ok(job) => {
                let utterance_id = job.utterance_id;
                let outcome = job.outcome;
                // `events()` covers both bearing arms: ordinary patches and the
                // gap-appends an under-commit recovered. Counting only `Patches`
                // here would drop recovered speech from the session summary.
                self.replacements = self.replacements.saturating_add(
                    outcome
                        .events()
                        .iter()
                        .filter(|event| matches!(event, EngineEvent::ReplaceRange { .. }))
                        .count() as u64,
                );
                if matches!(outcome, TailPatchOutcome::Skipped { .. }) {
                    self.skipped = self.skipped.saturating_add(1);
                }
                TailPatchCompletion {
                    utterance_id,
                    covered_through_secs: req_end_secs,
                    outcome,
                    payload: Some(job.payload),
                }
            }
            Err(error) => {
                self.skipped = self.skipped.saturating_add(1);
                TailPatchCompletion {
                    utterance_id: request_id,
                    covered_through_secs: req_end_secs,
                    outcome: TailPatchOutcome::skipped(
                        crate::stt::tail_patcher::SkipReasonCode::ProviderError,
                        format!("tail patch failed: {error}"),
                    ),
                    payload: None,
                }
            }
        }
    }

    /// How many bounded replacements Layer 1 landed this session — the number
    /// `SessionFinalised.layer_summary` reports.
    fn replacements(&self) -> u64 {
        self.replacements
    }

    /// How many jobs put nothing on the canvas (skipped or failed).
    fn skipped(&self) -> u64 {
        self.skipped
    }
}

/// Deliver one engine event to the sink, writing the same per-utterance
/// diagnostic line the VAD path writes.
///
/// Factored out because the Layer 1 branch must flush every queued event before
/// emitting a patch: a `ReplaceRange` that overtook its own `UtteranceFinal`
/// would address canvas that has not been committed yet.
fn deliver_event(
    event: &EngineEvent,
    event_sink: &dyn EventSink,
    stream_log_path: Option<&std::path::Path>,
) {
    if let (Some(path), EngineEvent::UtteranceFinal { text, .. }) = (stream_log_path, event) {
        let _ = append_to_stream_log(path, text.trim());
    }
    event_sink.on_event(event);
}

/// Surface one Layer 1 lane degrade as a counts-only warning event.
///
/// The message is the typed reason token and nothing else — no transcript,
/// audio, provider payload, or endpoint detail can ride this event into a log.
fn emit_layer1_degrade_warning(event_sink: &dyn EventSink, reason: Layer1DegradeReason) {
    event_sink.on_event(&EngineEvent::Warning {
        code: LAYER1_DEGRADED_WARNING_CODE.to_string(),
        message: reason.as_token().to_string(),
    });
}

/// Drive one progressive Apple stream session until the audio channel closes.
pub(crate) async fn apple_stream_transcription_session(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    event_sink: Arc<dyn EventSink>,
    config: SessionConfig,
) {
    let SessionConfig {
        sample_rate,
        language,
        stream_log_path,
        utterance_silence_sec,
        layer1,
        mut lifecycle_events,
    } = config;
    let mut capture_level = CaptureLevelAccumulator::new();
    // SFSpeech owns phrase boundaries in progressive mode, so the VAD-path
    // silence knob cannot apply. Say so instead of silently differing from
    // the `CODESCRIBE_APPLE_STT_LIVE_MODE=wav` escape hatch.
    if let Some(sec) = utterance_silence_sec {
        warn!(
            utterance_silence_sec = sec,
            "Apple progressive live mode ignores utterance_silence_sec \
             (SFSpeech decides phrase boundaries; use CODESCRIBE_APPLE_STT_LIVE_MODE=wav \
             for the VAD silence contract)"
        );
    }

    info!(
        sample_rate,
        "Apple progressive live session started (stream multi-seal)"
    );
    let session_id = uuid::Uuid::new_v4().to_string();

    // W13-1 inline-format buffer: arm a fresh chunk/chain session (no-op when
    // `CODESCRIBE_INLINE_FORMAT` is off). Must happen on the async side — the
    // blocking seal worker only ever enqueues sealed chunks.
    crate::llm::inline_format::begin_session(language.as_deref());

    // C1: open the injected Layer 1 lane at recording start. `Disarmed` is the
    // stock product (canvas + lexicon); an armed provider only ever arrives
    // here already authorized — construction and consent live with the
    // settings owner, not in this pipeline. Every lane failure from here on
    // degrades back to exactly the disarmed behavior.
    let lane_input = Layer1SessionInput {
        session_id: Layer1SessionId::new(session_id.clone())
            .expect("uuid session ids are never blank"),
        locale: language.clone(),
        sample_rate,
    };
    let mut layer1_lane = RecorderLayer1Lane::open(layer1, &lane_input);
    if let Some(reason) = layer1_lane.take_degrade_notice() {
        emit_layer1_degrade_warning(event_sink.as_ref(), reason);
    }

    // PCM → worker (None = EOF). Unbounded so the async select loop never
    // blocks on a full sync_channel while live Preview events wait to drain
    // (bounded sync_channel + blocking send would re-stall presentation).
    let (pcm_tx, pcm_rx) = std_mpsc::channel::<Option<Vec<f32>>>();
    // Worker → async events.
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<EngineEvent>();

    // Layer 1 (Whisper tail-patch) lane — off unless
    // `CODESCRIBE_LAYERED_TRANSCRIPTION=phase1+`. Read once here so the whole
    // session agrees on one answer even if the env flips mid-hold.
    let tail_patch_on = tail_patch_enabled();
    if tail_patch_on {
        info!(
            "Layered transcription Layer 1 (Whisper tail-patch) enabled on Apple progressive path"
        );
    }
    let mut tail_patch_lane = AppleTailPatchLane::new(sample_rate, language.clone());
    // At-most-one-in-flight gate (F1), tracked outside the lane so the admit
    // branch's guard does not borrow what the collect branch holds mutably.
    let mut tail_patch_in_flight = false;
    let mut tail_patch_lane_in_flight: Option<(u64, f32)> = None;
    // Bounded: the worker `try_send`s from the PCM-forwarding thread.
    let (tp_tx, mut tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
    let (tp_done_tx, tp_done_rx) = std_mpsc::channel::<TailPatchCompletion>();
    // Layered off → the worker gets no sender at all, so the lane stays empty
    // and its branch never yields: zero jobs, zero behaviour change.
    let worker_tp_tx = tail_patch_on.then_some(tp_tx);

    let worker_session_id = session_id.clone();
    let worker = thread::spawn(move || {
        apple_stream_worker(
            pcm_rx,
            ev_tx,
            sample_rate,
            language.as_deref(),
            worker_session_id,
            worker_tp_tx,
            tp_done_rx,
        )
    });

    // CRITICAL (operator 2026-07-27 — live preview "blocked" on overlay):
    // PCM forward and EngineEvent drain MUST interleave. The previous shape
    // drained `ev_rx` only *after* `chunk_receiver` closed (key-up / stop), so
    // every `Preview` / mid-stream `UtteranceFinal` sat in the unbounded queue
    // until EOF. The engine had letter-level partials; the overlay saw nothing
    // until the session ended. Product truth: presentation was missing, not STT.
    let mut audio_eof = false;
    loop {
        tokio::select! {
            event = ev_rx.recv() => {
                match event {
                    // Same diagnostic artifact the VAD path writes: one line
                    // per committed utterance (CODESCRIBE_STREAM_LOG).
                    Some(event) => deliver_event(
                        &event,
                        event_sink.as_ref(),
                        stream_log_path.as_deref(),
                    ),
                    // Worker dropped the sender — stream finished.
                    None => break,
                }
            }
            chunk = chunk_receiver.recv(), if !audio_eof => {
                match chunk {
                    Some(chunk) => {
                        capture_level.push_samples(&chunk);
                        // C1 fan-out: offer the frame to the Layer 1 lane
                        // before forwarding to the Apple worker. The offer
                        // returns immediately, always — a refiner that cannot
                        // keep up costs refinement frames, never capture, and
                        // sustained overflow degrades the lane instead of
                        // exerting backpressure here.
                        layer1_lane.offer_pcm(&chunk);
                        if pcm_tx.send(Some(chunk)).is_err() {
                            warn!("Apple live stream worker dropped PCM channel");
                            audio_eof = true;
                        }
                    }
                    None => {
                        // Capture stopped — signal EOF to the worker; keep
                        // draining events until the worker exits.
                        let _ = pcm_tx.send(None);
                        audio_eof = true;
                    }
                }
            }
            lifecycle = async {
                match lifecycle_events.as_mut() {
                    Some(events) => events.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match lifecycle {
                    Some(event) => {
                        apply_recorder_lifecycle_event(&mut layer1_lane, event);
                        if let Some(reason) = layer1_lane.take_degrade_notice() {
                            emit_layer1_degrade_warning(event_sink.as_ref(), reason);
                        }
                    }
                    None => lifecycle_events = None,
                }
            }
            // Admit one sealed utterance into Layer 1 at a time. The Whisper
            // call itself runs on `spawn_blocking` inside the job, so this loop
            // only ever schedules and collects — inference never sits on the
            // event-drain path (F1).
            Some(req) = tp_rx.recv(), if !tail_patch_in_flight => {
                let utterance_id = req.utterance_id;
                let covered_through_secs = req.covered_through_secs;
                tail_patch_lane.push_request(req);
                tail_patch_in_flight = true;
                // One job is in flight, so one end boundary is enough.
                // FuturesOrdered preserves the same request/result order.
                tail_patch_lane_in_flight = Some((utterance_id, covered_through_secs));
            }
            Some(result) = tail_patch_lane.next() => {
                tail_patch_in_flight = false;
                let (id, end) = tail_patch_lane_in_flight.take().unwrap_or_default();
                let completion = tail_patch_lane.finish_for_worker(id, end, result);
                let _ = tp_done_tx.send(completion);
            }
        }
        // C1: drain whatever the Layer 1 provider has ready. Partials stay
        // volatile draft inside the lane (never canvas); finals pass the
        // ingest doctrine. Non-blocking, so live Preview drainage above is
        // never delayed by the refiner.
        layer1_lane.poll();
        if let Some(reason) = layer1_lane.take_degrade_notice() {
            emit_layer1_degrade_warning(event_sink.as_ref(), reason);
        }
    }

    // Worker exited (event channel closed). If audio is still open, keep
    // consuming to EOF so upstream capture senders never hit a dropped
    // channel — an early engine death (e.g. bridge spawn failure) must not
    // turn live audio callbacks into send errors. Mirrors the pre-interleave
    // contract where the session always outlived the audio stream.
    //
    // This comes before the Layer 1 backlog on purpose: capture-sender safety
    // is the older, harder contract, and Whisper must never run while live
    // audio is still being drained.
    if !audio_eof {
        while chunk_receiver.recv().await.is_some() {}
    }

    // No new seals can arrive — but seals the worker emitted just before
    // exiting (the trailing summary, the open partial) may still be queued.
    // Whether the select loop admitted them before `ev_rx` closed is a race, so
    // finish the backlog here instead: a patch that lands only sometimes is
    // worse than one that always lands.
    //
    // This is bounded work, not a stop-time re-pass: at most `TAIL_PATCH_QUEUE_CAP`
    // already-sealed utterances, one job at a time, each emitting only bounded
    // `ReplaceRange` events. No Preview is waiting on it — capture is over.
    let mut settled_at_stop = 0u64;
    loop {
        while let Some(result) = tail_patch_lane.next().await {
            let (id, end) = tail_patch_lane_in_flight.take().unwrap_or_default();
            let completion = tail_patch_lane.finish_for_worker(id, end, result);
            let _ = tp_done_tx.send(completion);
        }
        match tp_rx.try_recv() {
            Ok(req) => {
                tail_patch_lane_in_flight = Some((req.utterance_id, req.covered_through_secs));
                tail_patch_lane.push_request(req);
                settled_at_stop = settled_at_stop.saturating_add(1);
            }
            Err(_) => break,
        }
    }
    if settled_at_stop > 0 {
        info!(
            settled_at_stop,
            "Layer 1 tail-patch backlog settled after capture stopped"
        );
    }

    // C1 stop-drain: close the Layer 1 lane with its bounded drain. Whatever
    // happened inside (clean close, disconnect, incomplete drain), the method
    // returns and the recording finishes on Apple + lexicon. The outcome's
    // finals are doctrine-vetted gap-fill candidates: their one road to
    // delivered text is `Layer1SessionOutcome::adjudicate_against_live_floor`
    // (the T0 `merge_live_layer1` seam), owned by the stop-path truth
    // adjudicator once the settings cut arms real providers.
    let layer1_outcome = layer1_lane.stop();
    if let Some(reason) = layer1_lane.take_degrade_notice() {
        emit_layer1_degrade_warning(event_sink.as_ref(), reason);
    }
    let layer1_counts = layer1_outcome.telemetry();
    if layer1_counts.frames_offered > 0 || layer1_counts.finals_accepted > 0 {
        info!(
            frames_forwarded = layer1_counts.frames_forwarded,
            overflow_frame_drops = layer1_counts.overflow_frame_drops,
            partials_applied = layer1_counts.partials_applied,
            finals_accepted = layer1_counts.finals_accepted,
            events_rejected = layer1_counts.events_rejected,
            provider_errors = layer1_counts.provider_errors,
            degrade_reason = layer1_outcome
                .degrade_reason()
                .map(|reason| reason.as_token())
                .unwrap_or("none"),
            "Layer 1 live lane closed"
        );
    }

    match worker.join() {
        Ok(Ok(outcome)) => {
            info!(
                sealed = outcome.sealed,
                filtered_empty_drops = outcome.filtered_empty_drops,
                unresolved_windows = outcome.unresolved_windows,
                under_commit_escalations = outcome.under_commit_escalations,
                "Apple progressive live session finished"
            );
        }
        Ok(Err(e)) => {
            warn!("Apple live stream worker failed: {e:#}");
            event_sink.on_event(&EngineEvent::NoSpeech {
                reason: format!("apple_live_stream_worker: {e:#}"),
            });
        }
        Err(_) => {
            warn!("Apple live stream worker panicked");
            event_sink.on_event(&EngineEvent::NoSpeech {
                reason: "apple_live_stream_worker_panic".into(),
            });
        }
    }

    log_tail_patch_session_receipt(tail_patch_lane.replacements(), tail_patch_lane.skipped());
    emit_capture_level_receipt(
        event_sink.as_ref(),
        &capture_level.finalize(CapturePathMeta::resolve(sample_rate, 1, None)),
    );
    emit_session_finalised(
        event_sink.as_ref(),
        session_id,
        tail_patch_lane.replacements(),
    );
}

/// Mutable seal state for one Apple stream: revision counters plus the shared
/// postprocessor that corrects every final at seal time.
///
/// Grouped into one struct so `emit_stream_events` keeps a readable signature
/// while the worker and the event mapper stay on the same postprocessor
/// instance (one lexicon reload cadence, one drop counter).
struct PendingAppleSeal {
    raw_text: String,
    start_ts: f32,
    end_ts: f32,
    segments: Vec<TranscriptSegment>,
}

struct AppleSealState {
    session_id: String,
    capture_epoch: u64,
    sample_rate: u32,
    postprocessor: StreamPostProcessor,
    preview_rev: u64,
    utterance_id: u64,
    open_partial: String,
    open_partial_segments: Vec<TranscriptSegment>,
    sealed_count: u64,
    filtered_empty_drops: u64,
    /// Bounded PCM retention, so a sealed boundary can be resolved back to the
    /// audio behind it (Layer 1 tail-patch prerequisite).
    audio: LiveAudioBuffer,
    /// Session time of the previous seal — the lower bound of the next
    /// utterance's audio window.
    last_sealed_end: f32,
    /// End of the last Apple segment admitted to committed canvas. Unlike the
    /// PCM retention cursor, this advances even when Layer 1 audio lookup is
    /// unavailable: Apple segment time is the authority for text disjointness.
    last_apple_segment_end: f32,
    /// Seals whose audio window could not be resolved (F3 falsification).
    unresolved_windows: u64,
    /// Seals where Layer 1 recovered speech it could not place on the canvas
    /// (W-C). A non-zero count means the stop path is owed a residual gap fill.
    under_commit_escalations: u64,
    /// Layer 1 hand-off, present only when layered transcription is armed.
    tail_patch: Option<mpsc::Sender<TailPatchRequest>>,
    /// Seals whose tail-patch request found the queue full (F1 backpressure).
    tail_patch_backpressure_drops: u64,
    /// Requests accepted by the Layer 1 queue that have not reported back yet.
    /// This — not the pending-seal queue — is what the end-of-session closure
    /// loop waits on: a span can also be held by the Apple volatile window, and
    /// no Whisper completion will ever clear that gate.
    tail_patch_awaiting_completion: u64,
    /// Concatenation of already progressive-sealed text — left context for
    /// Light+ casing on the next seal (w2-b).
    sealed_prefix: String,
    /// Canonical double-close authority used by the production Apple lane.
    progressive: ProgressiveSealMachine,
    /// Event payload retained until the machine declares the span sealed.
    pending_events: BTreeMap<u64, PendingAppleSeal>,
    /// Whisper outcomes retained until their final has been emitted.
    tail_patch_outcomes: BTreeMap<u64, TailPatchOutcome>,
    /// Silero Supervisor identity ledger. `None` unless the W13-3B lane flag
    /// is explicitly armed — default keeps Apple-boundary production identical.
    fusion: Option<SileroIngress>,
    fusion_context: FusionContextMode,
}

impl AppleSealState {
    /// Fresh seal state with Layer 1 disabled (`tail_patch: None`) — the default
    /// shape when `CODESCRIBE_LAYERED_TRANSCRIPTION` is unset.
    #[cfg(test)]
    fn new(sample_rate: u32) -> Self {
        Self::new_for_session(sample_rate, uuid::Uuid::new_v4().to_string())
    }

    fn new_for_session(sample_rate: u32, session_id: String) -> Self {
        Self {
            session_id,
            capture_epoch: 0,
            sample_rate,
            postprocessor: StreamPostProcessor::new(),
            preview_rev: 0,
            utterance_id: 0,
            open_partial: String::new(),
            open_partial_segments: Vec::new(),
            sealed_count: 0,
            filtered_empty_drops: 0,
            audio: LiveAudioBuffer::new(sample_rate, DEFAULT_RETENTION_SECS),
            last_sealed_end: 0.0,
            last_apple_segment_end: 0.0,
            unresolved_windows: 0,
            under_commit_escalations: 0,
            tail_patch: None,
            tail_patch_backpressure_drops: 0,
            tail_patch_awaiting_completion: 0,
            sealed_prefix: String::new(),
            progressive: ProgressiveSealMachine::new(),
            pending_events: BTreeMap::new(),
            tail_patch_outcomes: BTreeMap::new(),
            fusion: None,
            fusion_context: FusionContextMode::UtteranceOnly,
        }
    }

    /// Same state, armed with the Layer 1 hand-off. Holding the sender is what
    /// makes `seal_utterance_final` clone the committed text at all — with no
    /// wire there is nothing to diff against later.
    #[cfg(test)]
    fn new_with_tail_patch(sample_rate: u32, tail_patch: mpsc::Sender<TailPatchRequest>) -> Self {
        Self {
            tail_patch: Some(tail_patch),
            ..Self::new(sample_rate)
        }
    }

    fn new_with_tail_patch_for_session(
        sample_rate: u32,
        session_id: String,
        tail_patch: mpsc::Sender<TailPatchRequest>,
    ) -> Self {
        Self {
            tail_patch: Some(tail_patch),
            ..Self::new_for_session(sample_rate, session_id)
        }
    }

    /// Apply one elapsed Whisper window, then emit every newly double-closed
    /// span. Finals and their bounded patches share `ev_tx`, so ordering cannot
    /// invert on the async side.
    fn complete_whisper_window(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        completion: TailPatchCompletion,
        now_secs: f32,
    ) {
        let utterance_id = completion.utterance_id;
        self.tail_patch_awaiting_completion = self.tail_patch_awaiting_completion.saturating_sub(1);
        let (evidence, words) = completion.payload.map_or((None, Vec::new()), |payload| {
            (Some(payload.evidence), payload.segments)
        });
        let outcome = if self.fusion.is_some() {
            apply_conservative_fusion(self, ev_tx, utterance_id, &words, completion.outcome)
        } else {
            completion.outcome
        };
        self.tail_patch_outcomes.insert(utterance_id, outcome);
        self.progressive
            .note_whisper_window_elapsed_with_provenance(
                utterance_id,
                completion.covered_through_secs,
                evidence,
                words,
            );
        self.emit_ready_progressive_seals(ev_tx, now_secs);
    }

    fn emit_ready_progressive_seals(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        now_secs: f32,
    ) {
        let tick = self.progressive.try_seal(now_secs, false);
        self.emit_seal_tick(ev_tx, tick);
    }

    /// End-of-session drain: seal whatever the double-close gates still hold.
    ///
    /// Shares the emit path with the live tick on purpose — a span sealed at
    /// session end must reach the sink as the same `UtteranceFinal` (+ patches)
    /// a mid-session seal would, or the last utterance of every take would be
    /// delivered by a different route than all the others.
    fn seal_remaining_at_session_end(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        let tick = self.progressive.seal_remaining_at_session_end(false);
        self.emit_seal_tick(ev_tx, tick);
    }

    fn emit_seal_tick(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>, tick: SealTick) {
        for sealed in tick.newly_sealed {
            let Some(pending) = self.pending_events.remove(&sealed.id) else {
                warn!(
                    span_id = sealed.id,
                    "progressive seal missing retained Apple payload"
                );
                continue;
            };
            self.sealed_count = self.sealed_count.saturating_add(1);
            self.sealed_prefix = self.progressive.sealed_prefix();
            // Seal = "format now" signal (W13-1): a sealed span is byte-stable,
            // so the inline-format buffer may chunk-format it while dictation
            // continues. Sync + non-blocking; no-op unless the flag is armed.
            crate::llm::inline_format::on_chunk_sealed(sealed.id, &sealed.text);
            let _ = ev_tx.send(EngineEvent::UtteranceFinal {
                utterance_id: sealed.id,
                text: sealed.text,
                raw_text: pending.raw_text,
                start_ts: pending.start_ts,
                end_ts: pending.end_ts,
                segments: pending.segments,
                vad_speech_pct: None,
                avg_logprob: None,
                compression_ratio: None,
                quality_gate_dropped: false,
                confidence_flags: Vec::new(),
            });
            if let Some(outcome) = self.tail_patch_outcomes.remove(&sealed.id) {
                // Escalate before the appends so the starved-canvas signal is
                // never lost if a later send fails; both ride the same channel
                // as the final, so ordering after `UtteranceFinal` holds.
                if outcome.residual_required() {
                    self.under_commit_escalations = self.under_commit_escalations.saturating_add(1);
                    let _ = ev_tx.send(EngineEvent::Warning {
                        code: UNDER_COMMIT_WARNING_CODE.to_string(),
                        message: format!("residual gap fill required for utterance {}", sealed.id),
                    });
                }
                for event in outcome.into_events() {
                    let _ = ev_tx.send(event);
                }
            }
        }
    }
}

/// What the worker sealed, and what seal-time postprocess filtered away.
struct AppleStreamOutcome {
    sealed: u64,
    filtered_empty_drops: u64,
    unresolved_windows: u64,
    /// How many seals escalated an unplaceable Layer 1 under-commit (W-C).
    under_commit_escalations: u64,
}

/// Resolve a sealed utterance back to its audio span, then release what can
/// never be re-cut.
///
/// F3 (falsification): the tail-patch cuts exactly `window(prev_end, end_ts)`
/// and hands it to Whisper. If an Apple `end_ts` ever fails to address retained
/// audio — a clock that does not agree with the PCM timeline, or a boundary
/// older than the retention cap — that must be visible here, in the live path.
/// A silent miss would surface as canvas patched from the wrong audio, so an
/// unresolved boundary yields `None` and never reaches Layer 1.
fn resolve_sealed_audio_window(
    state: &mut AppleSealState,
    end_ts: f32,
) -> Option<ResolvedAudioWindow> {
    let from = state.last_sealed_end;
    match state.audio.window_with_range(from, end_ts) {
        Some(window) => {
            state.last_sealed_end = end_ts;
            // Everything before this utterance is committed canvas; no future
            // patch reaches back past it.
            state.audio.committed_through(from);
            tracing::debug!(
                from_secs = from,
                end_ts,
                window_samples = window.samples.len(),
                retained_samples = state.audio.len(),
                "Apple seal resolved to audio window"
            );
            Some(window)
        }
        None => {
            state.unresolved_windows = state.unresolved_windows.saturating_add(1);
            warn!(
                from_secs = from,
                end_ts,
                retained_start_secs = state.audio.retained_start_secs(),
                session_secs = state.audio.session_secs(),
                "Apple seal window unresolved — end_ts does not address retained audio"
            );
            None
        }
    }
}

fn seconds_to_captured_sample(seconds: f32, sample_rate: u32, captured_end: u64) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    ((seconds as f64 * sample_rate.max(1) as f64).round() as u64).min(captured_end)
}

fn apple_segments_on_pcm_clock(
    state: &AppleSealState,
    segments: &[TranscriptSegment],
) -> Vec<TimedTailSegment> {
    let captured_end = state.audio.session_sample_end();
    segments
        .iter()
        .map(|segment| {
            let sample_start =
                seconds_to_captured_sample(segment.start_ts, state.sample_rate, captured_end);
            let sample_end =
                seconds_to_captured_sample(segment.end_ts, state.sample_rate, captured_end)
                    .max(sample_start);
            TimedTailSegment {
                text: segment.text.clone(),
                range: TailSampleRange {
                    session: state.session_id.clone(),
                    capture_epoch: state.capture_epoch,
                    sample_start,
                    sample_end,
                },
            }
        })
        .collect()
}

/// Longest callback prefix the canvas already carries, tolerating the word
/// revisions SFSpeech makes when it re-states a phrase.
///
/// # Why exact substring matching was the repetition defect
///
/// Cumulative Apple finals do not merely extend the previous hypothesis — they
/// REVISE it ("szuty" → "skróty", "dokładnie" → "dokładność"). An exact
/// `canvas.contains(prefix)` probe is anchored at the callback's first word and
/// all-or-nothing, so one revised word anywhere in the prefix invalidates every
/// probe length at once and the whole restatement re-commits as "novel".
/// Measured on the 2026-08-12 18:44 take: 30 of 42 rescues matched exactly one
/// word, the delivery carried 72% of its words inside a repeated 6-gram, and
/// the production replay of the same WAV reproduced full-sentence re-commits
/// differing by a single word ([28]/[29]/[30] in the replay finals).
///
/// # Match rule
///
/// For the longest `k`, some canvas window must be within `allowed(k)` word
/// edits (substitution, insertion, deletion) of `probe[..k]`, where `allowed`
/// is 0 for `k ≤ 2` and `max(1, k/5)` (20%) beyond that. Edit distance rather
/// than positional comparison on purpose: revisions include insertions and
/// deletions ("spotkałem się" → "się", an interjected "a"), and under a
/// positional rule one inserted word shifts every later word and cascades into
/// wholesale mismatch — the verified replay showed 15–22-word restatements
/// collapsing to a 6-word match exactly this way. Short probes stay exact: at
/// one or two words a tolerated edit is not a revision, it is a different word.
///
/// One asymmetry is deliberate: the LAST word of the matched prefix must itself
/// align to a canvas word (match or substitution). Otherwise a trailing novel
/// word could be "deleted into" the match — "alpha beta revised" against a
/// canvas holding "alpha beta" is one deletion away as a whole, and treating
/// that as re-heard would demote genuinely new tail speech to the preview lane.
/// A trailing deletion therefore shortens `k` instead of costing an edit.
///
/// Only the canvas tail (`2 × probe len + 16` words) is searched — a
/// restatement re-states recent speech, and the bound keeps the DP cost flat
/// no matter how long the session canvas grows.
///
/// # Why fuzziness is safe in this branch
///
/// This runs only for finals whose every segment was consumed by the trusted
/// timing boundary — Apple itself asserts the audio was already judged. Genuine
/// new speech arrives with fresh segment timestamps and never enters here, so a
/// near-match against the canvas is a re-hearing, not the operator saying a
/// similar sentence twice.
///
/// Returns `(known_prefix_words, word_edits_in_the_match)`.
fn revision_tolerant_known_prefix(probe: &[String], canvas: &[&str]) -> (usize, usize) {
    if probe.is_empty() || canvas.is_empty() {
        return (0, 0);
    }
    let n = probe.len();
    let band = (n / 5).max(1);
    let tail_start = canvas.len().saturating_sub(2 * n + 16);
    let tail = &canvas[tail_start..];

    let allowed = |k: usize| if k <= 2 { 0 } else { (k / 5).max(1) };
    let mut best_k = 0usize;
    let mut best_edits = 0usize;

    // One banded edit-distance DP per window start: row `i` covers probe[..i],
    // column `j` the window tail[s..s+j]. For every prefix length the cheapest
    // window end is `min over j`, so a single pass scores all `k` at once.
    for s in 0..tail.len() {
        let jmax = (tail.len() - s).min(n + band);
        let mut prev: Vec<usize> = (0..=jmax).collect();
        for i in 1..=n {
            let mut current = vec![usize::MAX; jmax + 1];
            current[0] = i;
            // Best score whose final operation aligns probe[i-1] to a canvas
            // word — the only endings that may close a matched prefix (see the
            // trailing-deletion note in the doc comment).
            let mut aligned_end = usize::MAX;
            for j in 1..=jmax {
                // Outside the band the distance already exceeds every budget.
                if i.abs_diff(j) > band {
                    continue;
                }
                let substitute = if probe[i - 1] == tail[s + j - 1] {
                    prev[j - 1]
                } else {
                    prev[j - 1].saturating_add(1)
                };
                aligned_end = aligned_end.min(substitute);
                let delete = prev[j].saturating_add(1);
                let insert = current[j - 1].saturating_add(1);
                current[j] = substitute.min(delete).min(insert);
            }
            let edits = aligned_end;
            if edits <= allowed(i) && (i > best_k || (i == best_k && edits < best_edits)) {
                best_k = i;
                best_edits = edits;
            }
            prev = current;
        }
    }
    (best_k, best_edits)
}

/// Case- and punctuation-insensitive projection for canvas containment checks
/// (the sealed canvas carries Light+ casing and sentence terminals, raw
/// callbacks carry neither).
fn normalize_for_containment(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fuse Whisper words onto the pending Apple span through the rewrite fence.
///
/// Agreements and clear gap fills become the pending text. Unresolved
/// alternatives stay on Apple and emit a content-free receipt. The LCS
/// `ReplaceRange` outcome is dropped: the fused text is already in the span.
fn apply_conservative_fusion(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    utterance_id: u64,
    whisper_words: &[TimedTailSegment],
    fallback: TailPatchOutcome,
) -> TailPatchOutcome {
    let apple_words: Vec<FusionWord> = state
        .progressive
        .pending_spans()
        .iter()
        .find(|span| span.id == utterance_id)
        .map(|span| span.words.iter().map(FusionWord::from_timed).collect())
        .unwrap_or_default();
    let whisper: Vec<FusionWord> = whisper_words.iter().map(FusionWord::from_timed).collect();
    if apple_words.is_empty() && whisper.is_empty() {
        return fallback;
    }
    let decision = conservative_fuse(&apple_words, &whisper);
    if !state.progressive.try_rewrite(utterance_id, &decision.text) {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: SkipReasonCode::SealedFence.as_str().to_string(),
            message: format!("fusion rewrite refused for utterance {utterance_id}"),
        });
        return TailPatchOutcome::skipped(SkipReasonCode::SealedFence, "sealed_fence");
    }
    if !decision.unresolved.is_empty() {
        let receipt = fusion_receipt(utterance_id, &decision);
        let _ = ev_tx.send(EngineEvent::Warning {
            code: receipt.code.as_str().to_string(),
            message: format!(
                "fusion unresolved={} agreements={} gap_fills={}",
                receipt.unresolved, receipt.agreements, receipt.gap_fills
            ),
        });
    }
    TailPatchOutcome::NoChange
}

/// Slice a cumulative Apple final onto Silero-minted utterance ranges.
///
/// Returns `true` when at least one Silero span accepted words (the callback
/// is consumed). `false` leaves the caller on the Apple-boundary path so
/// speech is never dropped when Silero has not yet opened an edge.
fn seal_sliced_by_silero(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    raw_text: &str,
    after_lexicon: &str,
    start_ts: f32,
    end_ts: f32,
    disjoint: &[TranscriptSegment],
) -> bool {
    let Some(ledger) = state.fusion.as_ref().map(|fusion| fusion.ledger().clone()) else {
        return false;
    };
    if ledger.utterances().is_empty() {
        return false;
    }
    let apple_words = apple_segments_on_pcm_clock(state, disjoint);
    let fusion_words: Vec<FusionWord> = apple_words.iter().map(FusionWord::from_timed).collect();
    let (sliced, leftover) = slice_apple_words(&ledger, &fusion_words);
    if sliced.is_empty() {
        if !leftover.is_empty() {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: SkipReasonCode::NoTimeOverlap.as_str().to_string(),
                message: format!(
                    "apple words={} had no Silero utterance overlap",
                    leftover.len()
                ),
            });
        }
        return false;
    }
    if !leftover.is_empty() {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: SkipReasonCode::NoTimeOverlap.as_str().to_string(),
            message: format!(
                "apple leftover_words={} sliced_utterances={}",
                leftover.len(),
                sliced.len()
            ),
        });
    }

    let rate = state.sample_rate.max(1) as f32;
    let pad_samples = (super::silero_fusion::DEFAULT_LEFT_PAD_SECS * rate).round() as u64;
    let long_silence = (super::silero_fusion::LONG_SILENCE_FENCE_SECS * rate).round() as u64;
    let context = state.fusion_context;

    for (utterance_id, words) in sliced {
        let Some(silero) = ledger
            .utterances()
            .iter()
            .find(|utterance| utterance.id == utterance_id)
            .cloned()
        else {
            continue;
        };
        if !state.progressive.may_rewrite(utterance_id)
            && state
                .progressive
                .sealed_spans()
                .iter()
                .any(|span| span.id == utterance_id)
        {
            continue;
        }
        let text = words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let text = if text.trim().is_empty() {
            after_lexicon.to_string()
        } else {
            text
        };
        let span_start = words
            .first()
            .map(|word| word.sample_start as f32 / rate)
            .unwrap_or(start_ts);
        let span_end = words
            .last()
            .map(|word| word.sample_end as f32 / rate)
            .unwrap_or(end_ts);
        let timed: Vec<TimedTailSegment> = words
            .iter()
            .map(|word| TimedTailSegment {
                text: word.text.clone(),
                range: TailSampleRange {
                    session: state.session_id.clone(),
                    capture_epoch: state.capture_epoch,
                    sample_start: word.sample_start,
                    sample_end: word.sample_end,
                },
            })
            .collect();
        if state
            .progressive
            .pending_spans()
            .iter()
            .any(|p| p.id == utterance_id)
        {
            let _ = state.progressive.try_rewrite(utterance_id, &text);
        } else {
            if !state.progressive.note_apple_commit_timed(
                utterance_id,
                text.clone(),
                span_end,
                span_end,
                silero.range.clone(),
                timed,
                TailProviderEvidence {
                    source: TailEvidenceSource::AppleSpeech,
                    revision: None,
                    stability: TailEvidenceStability::Final,
                    timing_quality: TailTimingQuality::ExactSampleRange,
                    avg_logprob: None,
                },
            ) {
                continue;
            }
            state.pending_events.insert(
                utterance_id,
                PendingAppleSeal {
                    raw_text: raw_text.to_string(),
                    start_ts: span_start,
                    end_ts: span_end,
                    segments: disjoint.to_vec(),
                },
            );
        }

        let fence = ledger
            .utterances()
            .iter()
            .rev()
            .find(|prev| prev.closed && prev.range.sample_end <= silero.range.sample_start)
            .map(|prev| {
                let gap = silero
                    .range
                    .sample_start
                    .saturating_sub(prev.range.sample_end);
                if gap >= long_silence {
                    silero.range.sample_start
                } else {
                    0
                }
            })
            .unwrap_or(0);
        let request_range = bound_context_range(&silero.range, fence, context, pad_samples);
        let window = state
            .audio
            .window_by_samples(request_range.sample_start, request_range.sample_end);
        let queued = if let (Some(window), Some(tx)) = (window, state.tail_patch.as_ref()) {
            let committed_text = seal_span_text(&text, &state.sealed_prefix, false);
            match tx.try_send(TailPatchRequest {
                utterance_id,
                committed_text,
                audio: window.samples,
                provider_request: TailProviderRequest {
                    identity: TailRequestIdentity {
                        request_id: utterance_id,
                        range: TailSampleRange {
                            session: state.session_id.clone(),
                            capture_epoch: state.capture_epoch,
                            sample_start: window.sample_start,
                            sample_end: window.sample_end,
                        },
                    },
                    sample_rate: state.sample_rate,
                    language: None,
                },
                covered_through_secs: span_end,
            }) {
                Ok(()) => {
                    state.tail_patch_awaiting_completion =
                        state.tail_patch_awaiting_completion.saturating_add(1);
                    true
                }
                Err(error) => {
                    state.tail_patch_backpressure_drops =
                        state.tail_patch_backpressure_drops.saturating_add(1);
                    warn!(
                        utterance_id,
                        "Layer 1 tail-patch request dropped — queue full or lane gone: {error}"
                    );
                    false
                }
            }
        } else {
            false
        };
        if !queued {
            state
                .progressive
                .note_whisper_window_elapsed(utterance_id, span_end);
            state.emit_ready_progressive_seals(
                ev_tx,
                span_end + super::progressive_seal::APPLE_VOLATILE_WINDOW_SECS + 0.001,
            );
        }
        state.utterance_id = state.utterance_id.max(utterance_id);
    }
    true
}

/// Seal one Apple utterance: run the shared lexicon + cleanup pass, then emit
/// `UtteranceFinal`. Returns `false` when postprocess filtered the text to
/// empty — mirroring `PostprocessDrop::FilteredEmpty` on the VAD path, an
/// explicit `Drop` event is emitted instead of an empty final.
///
/// `raw_text` keeps the uncorrected engine output so the quality loop can see
/// exactly what the lexicon rewrote (same contract as the VAD path).
fn seal_utterance_final(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    raw: &str,
    segments: Vec<TranscriptSegment>,
    audio_secs: f32,
) -> bool {
    const BOUNDARY_EPSILON_SECS: f32 = 0.002;

    let callback_text = raw.trim().to_string();
    let original_segment_count = segments.len();
    let mut disjoint = Vec::with_capacity(original_segment_count);
    let mut cursor = state.last_apple_segment_end;
    let mut overlap_normalized = false;

    for mut segment in segments {
        let text = segment.text.trim();
        if text.is_empty()
            || !segment.start_ts.is_finite()
            || !segment.end_ts.is_finite()
            || segment.end_ts <= segment.start_ts
        {
            continue;
        }
        if segment.end_ts <= cursor + BOUNDARY_EPSILON_SECS
            || segment.start_ts < cursor - BOUNDARY_EPSILON_SECS
        {
            overlap_normalized = true;
            continue;
        }
        if segment.start_ts < cursor {
            segment.start_ts = cursor;
        }
        segment.text = text.to_string();
        cursor = segment.end_ts;
        disjoint.push(segment);
    }

    if overlap_normalized {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: APPLE_FINAL_OVERLAP_WARNING_CODE.to_string(),
            message: "Apple final overlap removed at segment boundary".to_string(),
        });
    }

    if disjoint.is_empty() {
        if callback_text.is_empty() {
            return false;
        }
        // Append doctrine: text Apple asserted must never die in the preview
        // lane — the next partial replaces `open_partial` wholesale and the
        // only copy is gone (session a5623d55, 2026-08-12). The trusted
        // timing boundary exists to dedupe RE-HEARD text, so demotion is only
        // legal for text already on the canvas. Cumulative callbacks re-state
        // the whole phrase, so the longest canvas-known prefix splits off and
        // only the NOVEL suffix commits, with a session-clock window (the
        // fallback the doc header always promised for segment-less finals).
        let mut canvas = state.progressive.sealed_prefix();
        for span in state.progressive.pending_spans() {
            canvas.push(' ');
            canvas.push_str(&span.raw_text);
        }
        let canvas = normalize_for_containment(&canvas);
        let words: Vec<&str> = callback_text.split_whitespace().collect();
        // Each probe word runs the lexicon, because the canvas already has:
        // `PendingSpan::raw_text` is `process_utterance` output (lexicon first)
        // and `sealed_prefix()` is `seal_span_text` output. Probing raw words
        // compares "doker" with "docker" and mismatches at every rewrite —
        // f8519df2 shipped exactly that and re-committed whole phrases;
        // `cumulative_final_prefix_survives_words_the_lexicon_rewrites` pins it.
        let probe_words: Vec<String> = words
            .iter()
            .map(|word| normalize_for_containment(&seal_span_text(word, "", true)))
            .collect();
        let canvas_words: Vec<&str> = canvas.split_whitespace().collect();
        let (known_prefix_words, revised_words) =
            revision_tolerant_known_prefix(&probe_words, &canvas_words);
        if revised_words > 0 {
            info!(
                known_prefix_words,
                revised_words,
                callback_words = words.len(),
                "apple_lifecycle: restated prefix matched through engine revisions"
            );
        }
        let novel_text = words[known_prefix_words..].join(" ");
        if novel_text.is_empty() {
            // Fully re-heard text: safe to keep as in-flight preview.
            state.open_partial = callback_text.clone();
            state.preview_rev = state.preview_rev.saturating_add(1);
            let _ = ev_tx.send(EngineEvent::Preview {
                rev: state.preview_rev,
                text: callback_text,
            });
            return false;
        }
        let start_ts = state.last_apple_segment_end.max(state.last_sealed_end);
        let end_ts = audio_secs.max(start_ts + BOUNDARY_EPSILON_SECS);
        info!(
            audio_secs,
            synthesized_start = start_ts,
            synthesized_end = end_ts,
            known_prefix_words,
            text_chars = novel_text.chars().count(),
            "apple_lifecycle: novel final suffix rescued with synthesized window"
        );
        disjoint.push(TranscriptSegment {
            text: novel_text,
            start_ts,
            end_ts,
        });
    }

    let start_ts = disjoint.first().map_or(0.0, |segment| segment.start_ts);
    let end_ts = disjoint.last().map_or(start_ts, |segment| segment.end_ts);
    let raw_text = if !overlap_normalized && disjoint.len() == original_segment_count {
        callback_text
    } else {
        disjoint
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    if raw_text.is_empty() {
        return false;
    }

    // Consume the Apple boundary even if cleanup filters the text. A later
    // cumulative callback must not resurrect audio the product already judged.
    state.last_apple_segment_end = end_ts;

    let Some(corrected) = state.postprocessor.process_utterance(&raw_text) else {
        state.filtered_empty_drops = state.filtered_empty_drops.saturating_add(1);
        warn!(
            raw_chars = raw_text.chars().count(),
            "Apple seal dropped: empty after lexicon/cleanup"
        );
        let _ = ev_tx.send(EngineEvent::Drop {
            kind: DropKind::FilteredEmpty,
            text: raw_text,
            reason: "Empty after lexicon/cleanup (not semantic gate)".to_string(),
        });
        return false;
    };

    let after_lexicon = corrected.trim().to_string();
    if state.fusion.is_some()
        && seal_sliced_by_silero(
            state,
            ev_tx,
            &raw_text,
            &after_lexicon,
            start_ts,
            end_ts,
            &disjoint,
        )
    {
        return true;
    }
    state.utterance_id = state.utterance_id.saturating_add(1);
    let utterance_id = state.utterance_id;
    let apple_words = apple_segments_on_pcm_clock(state, &disjoint);
    let captured_end = state.audio.session_sample_end();
    let span_sample_start = apple_words.first().map_or_else(
        || seconds_to_captured_sample(start_ts, state.sample_rate, captured_end),
        |word| word.range.sample_start,
    );
    let span_sample_end = apple_words.last().map_or_else(
        || seconds_to_captured_sample(end_ts, state.sample_rate, captured_end),
        |word| word.range.sample_end,
    );
    if !state.progressive.note_apple_commit_timed(
        utterance_id,
        after_lexicon.clone(),
        end_ts,
        end_ts,
        TailSampleRange {
            session: state.session_id.clone(),
            capture_epoch: state.capture_epoch,
            sample_start: span_sample_start,
            sample_end: span_sample_end.max(span_sample_start),
        },
        apple_words,
        TailProviderEvidence {
            source: TailEvidenceSource::AppleSpeech,
            revision: None,
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::ExactSampleRange,
            avg_logprob: None,
        },
    ) {
        return false;
    }
    state.pending_events.insert(
        utterance_id,
        PendingAppleSeal {
            raw_text,
            start_ts,
            end_ts,
            segments: disjoint,
        },
    );

    let window = resolve_sealed_audio_window(state, end_ts);
    let committed_text = seal_span_text(&after_lexicon, &state.sealed_prefix, false);
    let queued = if let (Some(window), Some(tx)) = (window, state.tail_patch.as_ref()) {
        let provider_request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: utterance_id,
                range: TailSampleRange {
                    session: state.session_id.clone(),
                    capture_epoch: state.capture_epoch,
                    sample_start: window.sample_start,
                    sample_end: window.sample_end,
                },
            },
            sample_rate: state.sample_rate,
            language: None,
        };
        match tx.try_send(TailPatchRequest {
            utterance_id,
            committed_text,
            audio: window.samples,
            provider_request,
            covered_through_secs: end_ts,
        }) {
            Ok(()) => {
                state.tail_patch_awaiting_completion =
                    state.tail_patch_awaiting_completion.saturating_add(1);
                true
            }
            Err(e) => {
                state.tail_patch_backpressure_drops =
                    state.tail_patch_backpressure_drops.saturating_add(1);
                warn!(
                    utterance_id,
                    "Layer 1 tail-patch request dropped — queue full or lane gone: {e}"
                );
                false
            }
        }
    } else {
        false
    };

    if !queued {
        // Layer 0/off and degraded queue paths must still deliver Apple text.
        // They are explicit fallbacks; the healthy layered path waits for the
        // actual Whisper completion above.
        state
            .progressive
            .note_whisper_window_elapsed(utterance_id, end_ts);
        state.emit_ready_progressive_seals(
            ev_tx,
            end_ts + super::progressive_seal::APPLE_VOLATILE_WINDOW_SECS + 0.001,
        );
    }
    true
}

/// Blocking worker: owns the stream session for its full lifetime.
fn apple_stream_worker(
    pcm_rx: std_mpsc::Receiver<Option<Vec<f32>>>,
    ev_tx: mpsc::UnboundedSender<EngineEvent>,
    sample_rate: u32,
    language: Option<&str>,
    session_id: String,
    tail_patch: Option<mpsc::Sender<TailPatchRequest>>,
    tail_patch_done: std_mpsc::Receiver<TailPatchCompletion>,
) -> anyhow::Result<AppleStreamOutcome> {
    let mut stream = LiveStreamSession::open(language, sample_rate)?;
    let mut state = match tail_patch {
        Some(tx) => AppleSealState::new_with_tail_patch_for_session(sample_rate, session_id, tx),
        None => AppleSealState::new_for_session(sample_rate, session_id),
    };
    if lane_enabled() {
        state.fusion = Some(SileroIngress::new(
            sample_rate,
            state.session_id.clone(),
            state.capture_epoch,
        ));
        state.fusion_context = FusionContextMode::from_env();
        info!(
            context = state.fusion_context.as_str(),
            "W13-3B Silero fusion lane armed (default remains off unless flagged)"
        );
    }
    let mut samples_seen: u64 = 0;

    loop {
        while let Ok(completion) = tail_patch_done.try_recv() {
            let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
            state.complete_whisper_window(&ev_tx, completion, audio_secs);
        }
        let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
        state.emit_ready_progressive_seals(&ev_tx, audio_secs);
        // Interleave PCM wait with progressive event polling so partials land
        // mid-utterance without waiting for the next audio chunk.
        match pcm_rx.recv_timeout(Duration::from_millis(40)) {
            Ok(Some(samples)) => {
                samples_seen += samples.len() as u64;
                // Retain before forwarding, on the same counter `audio_secs` is
                // derived from, so the buffer and the seal clock cannot drift.
                // This is worker-side on purpose: the async select loop stays
                // lock-free (2026-07-27 interleave contract) because the buffer
                // is never shared across the thread boundary.
                state.audio.push(&samples);
                if let Some(fusion) = state.fusion.as_mut() {
                    fusion.ingest(&samples, samples_seen);
                }
                stream.write_pcm(&samples)?;
                let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
                emit_stream_events(stream.poll_events(), &ev_tx, &mut state, audio_secs);
            }
            Ok(None) => break, // EOF from async side
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
                emit_stream_events(stream.poll_events(), &ev_tx, &mut state, audio_secs);
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
    if let Some(fusion) = state.fusion.as_mut() {
        fusion.flush(samples_seen);
    }
    let trailing = stream.finish()?;
    emit_stream_events(trailing, &ev_tx, &mut state, audio_secs);

    // Seal open partial that never got a phrase final (stop mid-phrase).
    // Same seal-time correction as the phrase path — a stop mid-utterance must
    // not be the one route that commits uncorrected text.
    let open = state.open_partial.trim().to_string();
    if !open.is_empty() {
        let segments = std::mem::take(&mut state.open_partial_segments);
        seal_utterance_final(&mut state, &ev_tx, &open, segments, audio_secs);
    }

    // Every accepted Layer 1 request must close (success, no-change, or
    // explicit skip) before the session task returns. This is bounded by the
    // queue cap and happens while the async side is still draining jobs.
    //
    // Wait on the *jobs*, not on the pending-seal queue. Those are different
    // conditions: a span still pending can be blocked by the Apple volatile
    // window rather than by a missing Whisper window, and no completion will
    // ever clear that gate. Waiting on the seal queue therefore parked the stop
    // path on the full timeout whenever the last span was volatile-blocked —
    // measured 2026-08-12, `rec_stop=36.701s` of which 30.005s was this loop
    // waiting for a completion that had already arrived for every job it sent.
    while state.tail_patch_awaiting_completion > 0 {
        match tail_patch_done.recv_timeout(TAIL_PATCH_CLOSURE_TIMEOUT) {
            Ok(completion) => state.complete_whisper_window(
                &ev_tx,
                completion,
                audio_secs + super::progressive_seal::APPLE_VOLATILE_WINDOW_SECS + 0.001,
            ),
            Err(error) => {
                warn!("progressive seal closure wait ended before all spans sealed: {error}");
                state.progressive.mark_live_lane_dead();
                break;
            }
        }
    }

    // Capture is over: no later Apple callback can revise a span and no further
    // Whisper window can arrive, so both double-close gates are satisfied by
    // definition. Seal the remainder here instead of leaving it to the residual
    // path — the machine's own span timestamps are the clock, because the audio
    // clock is frozen at EOF and can sit milliseconds behind them.
    state.seal_remaining_at_session_end(&ev_tx);

    // Evidence surface: when `CODESCRIBE_SEAL_ATLAS_DUMP` names a path, write
    // every sealed span with its PCM-pinned word payload as JSON. Runs on the
    // worker's own final state after the session-end seal, so the file is what
    // the session actually delivered — never a reconstruction. No env, no-op.
    if let Ok(dump_path) = std::env::var("CODESCRIBE_SEAL_ATLAS_DUMP") {
        if !dump_path.trim().is_empty() {
            let spans: Vec<serde_json::Value> = state
                .progressive
                .sealed_spans()
                .iter()
                .map(|span| {
                    serde_json::json!({
                        "id": span.id,
                        "text": span.text,
                        "end_secs_millis": span.end_secs_millis,
                        "range": span.range,
                        "words": span.words,
                        "apple_evidence": span.apple_evidence,
                        "whisper_evidence": span.whisper_evidence,
                        "whisper_words": span.whisper_words,
                    })
                })
                .collect();
            let atlas = serde_json::json!({
                "session": state.session_id,
                "capture_epoch": state.capture_epoch,
                "sample_rate": sample_rate,
                "audio_samples_seen": samples_seen,
                "sealed_spans": spans,
            });
            match serde_json::to_vec_pretty(&atlas)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| std::fs::write(&dump_path, bytes).map_err(anyhow::Error::from))
            {
                Ok(()) => info!(
                    path = %dump_path,
                    spans = state.progressive.sealed_spans().len(),
                    "seal atlas dump written"
                ),
                Err(error) => warn!(path = %dump_path, %error, "seal atlas dump failed"),
            }
        }
    }

    Ok(AppleStreamOutcome {
        sealed: state.sealed_count,
        filtered_empty_drops: state.filtered_empty_drops,
        unresolved_windows: state.unresolved_windows,
        under_commit_escalations: state.under_commit_escalations,
    })
}

/// Whether an open partial must be frozen before accepting a collapsed next
/// hypothesis from SFSpeech.
///
/// # Named drop mechanism: `shared_opener_restart_suppresses_freeze`
///
/// Measured 2026-08-10 three-way live (same mic/air): our committed raw lost
/// s6/s8/s10 while native Apple dictation kept them. Loss always followed a
/// stressor (fast speech / English terms / mumbling). Root cause at the
/// commit/adjudication layer: SFSpeech rarely emits `isFinal` on long Polish
/// dictation (13 restarts vs ONE isFinal on the 150 s fixture) and instead
/// collapses the open hypothesis onto the next sentence. Consecutive Polish
/// sentences share openers (`Zdanie` / `Zadanie`). The previous freeze rule
/// treated `prev.hasPrefix(next)` / substring containment as "extends", so a
/// collapse onto a short shared opener overwrote the prior utterance without
/// sealing it.
///
/// Freeze whenever `next` does not retain `prev` in full. The old restart
/// thresholds classify telemetry only; revision and same-phrase rewind are
/// retained too because this call site otherwise overwrites the only copy.
/// Kept in lockstep with `SfSpeechPhraseAccumulator` in the Swift bridge.
pub(crate) fn phrase_restart_should_freeze_prior(prev: &str, next: &str) -> bool {
    phrase_retention_reason(prev, next).is_some()
}

/// Telemetry classification for a retention decision. Text safety depends
/// only on forward containment, never on the restart/revision classifier.
fn phrase_retention_reason(prev: &str, next: &str) -> Option<&'static str> {
    let prev = prev.trim();
    let next = next.trim();
    if prev.is_empty() || next.contains(prev) {
        return None;
    }
    if next.is_empty() {
        return Some("empty_collapse_retained");
    }
    let prev_chars = prev.chars().count();
    let next_chars = next.chars().count();
    let restarted = (next_chars * 3 < prev_chars) || (next_chars <= 15 && prev_chars >= 25);
    Some(if restarted {
        "restart_retained"
    } else {
        "revision_retained"
    })
}

/// Map one poll's worth of bridge events onto `EngineEvent`s, sealing where the
/// stream says a phrase closed.
///
/// The mapping is where the RAW-preview / corrected-seal split is enforced:
/// `Partial` forwards verbatim, `PhraseFinal` goes through
/// [`seal_utterance_final`] (lexicon + cleanup). `audio_secs` is the session
/// clock and only acts as a fallback `end_ts` when the engine hands over no
/// segments. `Summary` is the partials-only engines' single seal — it commits
/// only when no phrase final ever arrived, otherwise it would double-seal what
/// the phrase path already committed.
///
/// On `Partial`, a collapsed post-stressor restart freezes the open hypothesis
/// first ([`phrase_restart_should_freeze_prior`]) so a shared-opener rewrite
/// cannot eat a whole utterance before the bridge emits a `final`.
fn emit_stream_events(
    events: Vec<LiveStreamEvent>,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    state: &mut AppleSealState,
    audio_secs: f32,
) {
    for event in events {
        match event {
            LiveStreamEvent::Ready => {
                info!(
                    audio_secs,
                    sealed = state.sealed_count,
                    "apple_lifecycle: recognizer ready / stream start"
                );
            }
            LiveStreamEvent::End => {
                info!(
                    audio_secs,
                    sealed = state.sealed_count,
                    open_partial_chars = state.open_partial.len(),
                    filtered_empty_drops = state.filtered_empty_drops,
                    "apple_lifecycle: recognizer end"
                );
            }
            LiveStreamEvent::Partial { text, segments } => {
                // Safety net for the named drop mechanism: if the bridge
                // missed a freeze (shared opener collapse), seal the open
                // partial here before the rewrite lands.
                if phrase_restart_should_freeze_prior(&state.open_partial, &text) {
                    let reason = phrase_retention_reason(&state.open_partial, &text)
                        .expect("freeze decision must carry a telemetry reason");
                    let frozen = state.open_partial.clone();
                    info!(
                        audio_secs,
                        prev_chars = frozen.chars().count(),
                        next_chars = text.chars().count(),
                        reason,
                        "apple_lifecycle: freeze open partial before restart partial"
                    );
                    let frozen_segments = std::mem::take(&mut state.open_partial_segments);
                    seal_utterance_final(state, ev_tx, &frozen, frozen_segments, audio_secs);
                    state.open_partial.clear();
                }
                // Previews stay RAW: they are in-flight presentation, not
                // canvas, and correcting them would make the lexicon rewrite
                // flicker letter by letter while the phrase is still forming.
                state.open_partial = text.clone();
                state.open_partial_segments = segments;
                state.progressive.note_session_partial(&text, audio_secs);
                state.preview_rev = state.preview_rev.saturating_add(1);
                let _ = ev_tx.send(EngineEvent::Preview {
                    rev: state.preview_rev,
                    text,
                });
            }
            LiveStreamEvent::PhraseFinal { text, segments } => {
                // The phrase is closed either way — the open partial is stale.
                info!(
                    audio_secs,
                    sealed_before = state.sealed_count,
                    text_chars = text.len(),
                    "apple_lifecycle: phrase final received"
                );
                state.open_partial.clear();
                state.open_partial_segments.clear();
                let committed = seal_utterance_final(state, ev_tx, &text, segments, audio_secs);
                info!(
                    audio_secs,
                    committed,
                    sealed_after = state.sealed_count,
                    "apple_lifecycle: phrase final adjudicated"
                );
            }
            LiveStreamEvent::Error { message } => {
                warn!("Apple live stream error event: {message}");
                let _ = ev_tx.send(EngineEvent::NoSpeech {
                    reason: format!("apple_live_stream: {message}"),
                });
            }
            LiveStreamEvent::Summary {
                text,
                segments,
                ok,
                error,
            } => {
                if !ok {
                    let msg = error.unwrap_or_else(|| "stream summary not ok".into());
                    warn!("Apple live stream summary error: {msg}");
                    let _ = ev_tx.send(EngineEvent::NoSpeech {
                        reason: format!("apple_live_stream_summary: {msg}"),
                    });
                    continue;
                }
                // No phrase finals → seal the full summary once (partials-only engine).
                if state.utterance_id == 0 {
                    if seal_utterance_final(state, ev_tx, &text, segments, audio_secs) {
                        state.open_partial.clear();
                        state.open_partial_segments.clear();
                    }
                } else {
                    // Phrase seals already emitted; don't double-seal open partial.
                    state.open_partial.clear();
                    state.open_partial_segments.clear();
                }
            }
        }
    }
}

/// Seal mapping, lexicon-at-seal, retained PCM windows, and Layer 1 wiring.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contracts::LayerSource;
    use crate::stt::apple_stt::parse_stream_stdout_line;
    use crate::stt::tail_patcher::{LAYERED_TRANSCRIPTION_ENV, compute_tail_patch};
    use serial_test::serial;
    use std::ffi::OsString;
    use std::sync::Mutex;

    /// Capture rate the Apple bridge is opened with; these tests exercise seal
    /// text, not audio retention, so any valid rate is representative.
    const TEST_SAMPLE_RATE: u32 = 16_000;

    fn synthetic_tail_payload(
        request_id: u64,
        range: TailSampleRange,
        segments: Vec<TimedTailSegment>,
    ) -> TailProviderPayload {
        TailProviderPayload {
            identity: TailRequestIdentity { request_id, range },
            text: String::new(),
            segments,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            provider_id: crate::stt::tail_provider::TailProviderId::Fake,
            elapsed_ms: 0,
            evidence: TailProviderEvidence {
                source: TailEvidenceSource::Whisper,
                revision: Some("synthetic-test".to_string()),
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: None,
            },
        }
    }

    /// Integration boundary: the real Apple state owns the progressive
    /// machine. Apple commit + live partial alone stay pending; an elapsed
    /// Whisper window seals before session end.
    #[test]
    fn apple_session_progressive_machine_seals_after_whisper_before_session_end() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (tp_tx, _tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
        let mut state = AppleSealState::new_with_tail_patch(TEST_SAMPLE_RATE, tp_tx);
        push_capture(&mut state, 10.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "uruchom doker".into(),
                    segments: vec![segment("uruchom doker", 0.5, 2.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "uruchom doker".into(),
                    segments: vec![segment("uruchom doker", 0.5, 2.0)],
                },
            ],
            &tx,
            &mut state,
            2.0,
        );

        assert_eq!(state.progressive.pending_spans().len(), 1);
        let mut before = Vec::new();
        while let Ok(event) = rx.try_recv() {
            before.push(event);
        }
        assert!(
            before
                .iter()
                .all(|event| !matches!(event, EngineEvent::UtteranceFinal { .. })),
            "Apple commit alone must not bypass the double-seal condition"
        );

        let whisper_range = state.progressive.pending_spans()[0].range.clone();
        let whisper_word = TimedTailSegment {
            text: "doker".to_string(),
            range: whisper_range.clone(),
        };
        state.complete_whisper_window(
            &tx,
            TailPatchCompletion {
                utterance_id: 1,
                covered_through_secs: 2.0,
                outcome: TailPatchOutcome::NoChange,
                payload: Some(synthetic_tail_payload(1, whisper_range, vec![whisper_word])),
            },
            5.0,
        );

        let mut after = Vec::new();
        while let Ok(event) = rx.try_recv() {
            after.push(event);
        }
        assert!(
            after
                .iter()
                .any(|event| matches!(event, EngineEvent::UtteranceFinal { .. })),
            "double-closed span must seal live"
        );
        assert_eq!(state.progressive.sealed_spans().len(), 1);
        let sealed = &state.progressive.sealed_spans()[0];
        assert_eq!(sealed.range.sample_start, 8_000);
        assert_eq!(sealed.range.sample_end, 32_000);
        assert_eq!(sealed.words.len(), 1);
        assert_eq!(sealed.words[0].range, sealed.range);
        assert_eq!(
            sealed.apple_evidence.source,
            TailEvidenceSource::AppleSpeech
        );
        assert_eq!(
            sealed
                .whisper_evidence
                .as_ref()
                .map(|evidence| evidence.source),
            Some(TailEvidenceSource::Whisper)
        );
        assert_eq!(sealed.whisper_words.len(), 1);
    }

    /// W-C: an under-commit's gap-appends reach the canvas through the same
    /// seal gate as ordinary patches — strictly after `UtteranceFinal` — and an
    /// unplaceable remainder rides out as the residual escalation.
    #[test]
    fn apple_seal_emits_under_commit_gap_appends_and_escalation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (tp_tx, _tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
        let mut state = AppleSealState::new_with_tail_patch(TEST_SAMPLE_RATE, tp_tx);
        push_capture(&mut state, 10.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "raz dwa trzy cztery piec".into(),
                    segments: vec![segment("raz dwa trzy cztery piec", 0.5, 2.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "raz dwa trzy cztery piec".into(),
                    segments: vec![segment("raz dwa trzy cztery piec", 0.5, 2.0)],
                },
            ],
            &tx,
            &mut state,
            2.0,
        );
        while rx.try_recv().is_ok() {}

        // Whisper recovered a tail that only partly has a safe anchor.
        let outcome = compute_tail_patch(
            "raz dwa trzy cztery piec szesc",
            "raz dwa trzy alfa beta gamma cztery piec siedem osiem dziewiec dziesiec",
            1,
            &TailPatchConfig::default(),
        );
        assert!(
            outcome.residual_required(),
            "fixture must carry an unplaceable remainder"
        );
        state.complete_whisper_window(
            &tx,
            TailPatchCompletion {
                utterance_id: 1,
                covered_through_secs: 2.0,
                outcome,
                payload: None,
            },
            5.0,
        );

        let mut after = Vec::new();
        while let Ok(event) = rx.try_recv() {
            after.push(event);
        }
        let final_at = after
            .iter()
            .position(|e| matches!(e, EngineEvent::UtteranceFinal { .. }))
            .expect("span must seal");
        let patch_at = after
            .iter()
            .position(|e| {
                matches!(
                    e,
                    EngineEvent::ReplaceRange {
                        source: LayerSource::TailPatch,
                        ..
                    }
                )
            })
            .expect("recovered gap must reach the canvas, not be discarded");
        assert!(
            final_at < patch_at,
            "a gap-append must never overtake the final it addresses"
        );
        assert!(
            after.iter().any(|e| matches!(
                e,
                EngineEvent::Warning { code, .. } if code == UNDER_COMMIT_WARNING_CODE
            )),
            "unplaceable recovered speech must escalate outward"
        );
        assert_eq!(state.under_commit_escalations, 1);
    }

    /// Partial → Preview; each phrase final → UtteranceFinal with rising ids.
    #[test]
    fn emit_maps_partial_and_two_phrase_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "hello".into(),
                    segments: vec![segment("hello", 0.0, 0.5)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "hello world".into(),
                    segments: vec![segment("hello world", 0.0, 1.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "second".into(),
                    segments: vec![segment("second", 1.0, 2.0)],
                },
            ],
            &tx,
            &mut state,
            1.0,
        );
        drop(tx);
        let mut got = Vec::new();
        while let Ok(e) = rx.try_recv() {
            got.push(e);
        }
        assert_eq!(state.sealed_count, 2);
        assert!(matches!(got[0], EngineEvent::Preview { rev: 1, .. }));
        assert!(matches!(
            got[1],
            EngineEvent::UtteranceFinal {
                utterance_id: 1,
                ..
            }
        ));
        assert!(matches!(
            got[2],
            EngineEvent::UtteranceFinal {
                utterance_id: 2,
                ..
            }
        ));
    }

    /// Build a timed `TranscriptSegment` for seal-window fixture events.
    fn segment(text: &str, start_ts: f32, end_ts: f32) -> TranscriptSegment {
        TranscriptSegment {
            text: text.to_string(),
            start_ts,
            end_ts,
        }
    }

    /// Feed `secs` of captured audio the way the worker does — chunk by chunk.
    fn push_capture(state: &mut AppleSealState, secs: f32) {
        let total = (secs * TEST_SAMPLE_RATE as f32) as usize;
        let session = vec![0.25f32; total];
        for chunk in session.chunks(1024) {
            state.audio.push(chunk);
        }
    }

    /// Append doctrine (session a5623d55, 2026-08-12): a phrase final whose
    /// segments are entirely consumed by the trusted timing boundary but whose
    /// text carries NOVEL content must still reach the canvas. Demoting it to
    /// the preview lane is a silent replacement channel — the very next
    /// partial overwrites `open_partial` wholesale and the only copy dies.
    #[test]
    fn boundary_consumed_final_with_novel_text_still_reaches_canvas() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 40.0);

        // Utterance 1 commits normally; trusted boundary moves to 14.0.
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "Zmienili zobacz".into(),
                segments: vec![segment("Zmienili zobacz", 0.5, 14.0)],
            }],
            &tx,
            &mut state,
            14.2,
        );

        // SFSpeech restart re-delivers with stale timings BEHIND the boundary
        // but novel words; the collapsed restart partial lands right after.
        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "Czyli dupa zbita".into(),
                    segments: vec![segment("Czyli dupa zbita", 10.0, 13.5)],
                },
                LiveStreamEvent::Partial {
                    text: "Tak".into(),
                    segments: vec![segment("Tak", 17.0, 17.4)],
                },
            ],
            &tx,
            &mut state,
            17.5,
        );

        let mut finals = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::UtteranceFinal { text, .. } = event {
                finals.push(text);
            }
        }
        let canvas = finals.join(" ");
        assert!(
            canvas.contains("Czyli dupa zbita"),
            "Apple-asserted novel text died in the preview lane (podmianka): canvas={canvas:?}"
        );
    }

    /// Append doctrine, freeze path: the safety-net freeze seals the open
    /// partial WITHOUT segments. That seal must not die on `disjoint.is_empty()`
    /// — the frozen text is the only copy of a whole utterance.
    #[test]
    fn frozen_partial_without_segments_still_reaches_canvas() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 30.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "pojebany tekst czyli dupa".into(),
                    segments: Vec::new(),
                },
                // Collapsed restart: freeze must seal the prior hypothesis.
                LiveStreamEvent::Partial {
                    text: "Tak".into(),
                    segments: Vec::new(),
                },
            ],
            &tx,
            &mut state,
            12.0,
        );

        let mut finals = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::UtteranceFinal { text, .. } = event {
                finals.push(text);
            }
        }
        let canvas = normalize_for_containment(&finals.join(" "));
        assert!(
            canvas.contains("pojebany tekst czyli dupa"),
            "frozen open partial died sealing without segments: canvas={canvas:?}"
        );
    }

    /// F3 wiring contract: a seal must resolve to the audio actually retained
    /// for this session, and advance the lower bound for the next utterance.
    /// This is what W2-A's tail-patch will stand on.
    #[test]
    fn seals_resolve_their_audio_window_from_retained_pcm() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 6.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "pierwsze zdanie".into(),
                    segments: vec![segment("pierwsze zdanie", 0.5, 2.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "drugie zdanie".into(),
                    segments: vec![segment("drugie zdanie", 2.5, 4.0)],
                },
            ],
            &tx,
            &mut state,
            6.0,
        );

        assert_eq!(state.sealed_count, 2);
        assert_eq!(
            state.unresolved_windows, 0,
            "both boundaries must address retained audio"
        );
        assert_eq!(state.last_sealed_end, 4.0);
        // Audio before the last boundary is committed canvas and released.
        assert!(state.audio.window(0.0, 1.0).is_none());
        assert!(state.audio.window(2.5, 4.0).is_some());
    }

    #[test]
    fn cumulative_apple_final_commits_only_segments_after_last_boundary() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 4.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "alpha beta".into(),
                    segments: vec![segment("alpha", 0.0, 1.0), segment("beta", 1.0, 2.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "alpha beta gamma".into(),
                    segments: vec![
                        segment("alpha", 0.0, 1.0),
                        segment("beta", 1.0, 2.0),
                        segment("gamma", 2.0, 3.0),
                    ],
                },
            ],
            &tx,
            &mut state,
            4.0,
        );

        let mut finals = Vec::new();
        let mut overlap_warnings = 0;
        while let Ok(event) = rx.try_recv() {
            match event {
                EngineEvent::UtteranceFinal {
                    raw_text,
                    start_ts,
                    end_ts,
                    ..
                } => finals.push((raw_text, start_ts, end_ts)),
                EngineEvent::Warning { code, .. } if code == APPLE_FINAL_OVERLAP_WARNING_CODE => {
                    overlap_warnings += 1;
                }
                _ => {}
            }
        }
        assert_eq!(
            finals,
            vec![("alpha beta".into(), 0.0, 2.0), ("gamma".into(), 2.0, 3.0)]
        );
        assert_eq!(overlap_warnings, 1);
    }

    #[test]
    fn cumulative_final_commits_only_its_novel_suffix() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 3.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "alpha beta".into(),
                    segments: vec![segment("alpha", 0.0, 1.0), segment("beta", 1.0, 2.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "alpha beta revised".into(),
                    segments: vec![segment("alpha beta revised", 0.0, 2.0)],
                },
            ],
            &tx,
            &mut state,
            3.0,
        );

        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        // Append doctrine: the canvas-known prefix "alpha beta" must not
        // double-commit, but the novel suffix must never die in preview.
        let finals: Vec<&String> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(finals.len(), 2, "novel suffix must commit: {events:?}");
        assert!(
            normalize_for_containment(finals[1]).contains("revised"),
            "second final must carry only the novel suffix: {finals:?}"
        );
        assert!(
            !normalize_for_containment(finals[1]).contains("alpha"),
            "canvas-known prefix must not double-commit: {finals:?}"
        );
        assert_eq!(state.utterance_id, 2, "novel suffix gets a fresh ID");
        assert_eq!(
            state.last_apple_segment_end, 3.0,
            "synthesized window consumes the boundary to the session clock"
        );
    }

    /// Regression guard for the prefix probe: the canvas-known prefix must be
    /// recognised even when the lexicon rewrites words inside it.
    ///
    /// `cumulative_final_commits_only_its_novel_suffix` cannot catch this — its
    /// "alpha beta revised" survives every rewrite table untouched, so it stayed
    /// green through the whole defect. Here "doker" → "Docker" puts a real
    /// rewrite inside the shared prefix, which is what broke the match: the
    /// probe was normalised through `seal_span_text` while the canvas is built
    /// from post-`process_utterance` text, so the two sides disagreed at the
    /// first rewritten word and nearly the whole phrase re-committed as novel.
    /// Measured on session f72fbbb7 (2026-08-12): 603 live words against 318
    /// spoken, +90%.
    #[test]
    fn cumulative_final_prefix_survives_words_the_lexicon_rewrites() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 3.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "uruchom doker".into(),
                    segments: vec![segment("uruchom", 0.0, 1.0), segment("doker", 1.0, 2.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "uruchom doker i restart".into(),
                    segments: vec![segment("uruchom doker i restart", 0.0, 2.0)],
                },
            ],
            &tx,
            &mut state,
            3.0,
        );

        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let finals: Vec<&String> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(
            finals.len(),
            2,
            "the novel suffix must still commit: {events:?}"
        );

        let novel = normalize_for_containment(finals[1]);
        assert!(
            novel.contains("restart"),
            "novel suffix must reach the canvas: {finals:?}"
        );
        assert!(
            !novel.contains("uruchom"),
            "a rewritten prefix is still a known prefix — re-committing it is the repetition defect: {finals:?}"
        );
        assert!(
            !novel.contains("doker") && !novel.contains("docker"),
            "the WHOLE known prefix must be consumed, not just the words the lexicon left alone — \
             stopping at the first rewritten word is exactly how a phrase re-commits: {finals:?}"
        );
    }

    /// Threshold contract of the fuzzy prefix: short probes stay exact, longer
    /// ones absorb ~20% revisions. At one or two words a tolerated mismatch is
    /// not a revision, it is a different word — loosening that end would let
    /// any two-word opener "match" the canvas and silently eat real speech.
    #[test]
    fn revision_tolerance_is_zero_for_short_probes_and_bounded_after() {
        let canvas = vec!["ala", "ma", "kota", "i", "psa"];
        let one = |s: &str| vec![s.to_string()];
        let owned = |words: &[&str]| words.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();

        // k=1..2: exact only.
        assert_eq!(revision_tolerant_known_prefix(&one("ala"), &canvas), (1, 0));
        assert_eq!(revision_tolerant_known_prefix(&one("ela"), &canvas), (0, 0));
        assert_eq!(
            revision_tolerant_known_prefix(&owned(&["ela", "ma"]), &canvas),
            (0, 0),
            "a two-word probe with a revision must NOT match — that is a different phrase"
        );

        // k=3: one revision allowed ("ela" for "ala").
        assert_eq!(
            revision_tolerant_known_prefix(&owned(&["ela", "ma", "kota"]), &canvas),
            (3, 1)
        );
        // Two revisions in three words: too different.
        assert_eq!(
            revision_tolerant_known_prefix(&owned(&["ela", "je", "kota"]), &canvas),
            (0, 0)
        );
        // Full exact run wins with zero revisions.
        assert_eq!(
            revision_tolerant_known_prefix(&owned(&["ala", "ma", "kota", "i", "psa"]), &canvas),
            (5, 0)
        );

        // Insertions and deletions are revisions too: a positional rule would
        // cascade every word after the shift into a mismatch. Measured on the
        // 2026-08-12 replay — 15-22-word restatements collapsed to a 6-word
        // match because Apple interjected or dropped a single word mid-phrase.
        assert_eq!(
            revision_tolerant_known_prefix(&owned(&["ala", "ma", "dużego", "kota", "i"]), &canvas),
            (5, 1),
            "one inserted word must cost one edit, not shift-poison the rest"
        );
        assert_eq!(
            revision_tolerant_known_prefix(&owned(&["ala", "kota", "i", "psa"]), &canvas),
            (4, 1),
            "one dropped word must cost one edit"
        );
    }

    /// The 2026-08-12 18:44 repetition mechanism, pinned: a cumulative final
    /// that REVISES its opening word ("szuty" → "skróty") used to defeat every
    /// probe length at once, because the exact-substring prefix match was
    /// anchored at the callback's first word. The whole restatement then
    /// re-committed — the delivered take carried 72% of its words inside a
    /// repeated 6-gram. `revision_tolerant_known_prefix` absorbs the revision.
    #[test]
    fn cumulative_final_with_revised_opening_word_must_not_recommit_the_phrase() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 40.0);

        let heard_first = "szuty klawiszowe to podwójny lewy przycisk myszy";
        let restated =
            "skróty klawiszowe to podwójny lewy przycisk myszy lub klawisz na klawiaturze";

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: heard_first.into(),
                segments: vec![segment(heard_first, 0.0, 5.0)],
            }],
            &tx,
            &mut state,
            40.0,
        );
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: restated.into(),
                segments: vec![segment(restated, 0.0, 6.0)],
            }],
            &tx,
            &mut state,
            40.0,
        );

        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let finals: Vec<&String> = events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::UtteranceFinal { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        let all = finals
            .iter()
            .map(|t| normalize_for_containment(t))
            .collect::<Vec<_>>()
            .join(" ");
        let count = all.matches("lewy przycisk myszy").count();
        assert_eq!(
            count, 1,
            "a restatement with one revised opening word must not re-commit the whole phrase: {finals:?}"
        );
    }

    #[test]
    fn legitimate_repeated_words_survive_disjoint_apple_windows() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 3.0);
        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "tak".into(),
                    segments: vec![segment("tak", 0.0, 1.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "tak".into(),
                    segments: vec![segment("tak", 1.0, 2.0)],
                },
            ],
            &tx,
            &mut state,
            3.0,
        );
        let raw_finals = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { raw_text, .. } => Some(raw_text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(raw_finals, vec!["tak", "tak"]);
    }

    /// Falsification arm: an `end_ts` that does not describe this session's PCM
    /// must be counted and surfaced, never silently truncated into a window.
    #[test]
    fn seal_window_beyond_captured_audio_is_counted_unresolved() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 2.0);

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "zdanie z przyszlosci".into(),
                segments: vec![segment("zdanie z przyszlosci", 8.0, 9.0)],
            }],
            &tx,
            &mut state,
            2.0,
        );

        assert_eq!(state.sealed_count, 1, "the text still seals");
        assert_eq!(state.unresolved_windows, 1);
        assert_eq!(
            state.last_sealed_end, 0.0,
            "an unresolved boundary must not advance the window floor"
        );
    }

    /// Contract sensor: mid-stream Previews must be consumable without waiting
    /// for audio EOF. The session select loop is the production path; this
    /// locks the interleave contract — events already queued while PCM is
    /// still open surface to the sink immediately (not only after stop).
    #[tokio::test]
    async fn live_previews_surface_before_audio_eof() {
        /// Test sink that records Preview text only (order of live surface).
        struct CollectSink(Mutex<Vec<String>>);
        impl EventSink for CollectSink {
            /// Append preview text when present; ignore non-preview events.
            fn on_event(&self, event: &EngineEvent) {
                if let EngineEvent::Preview { text, .. } = event {
                    self.0.lock().expect("lock").push(text.clone());
                }
            }
        }

        let sink = CollectSink(Mutex::new(Vec::new()));
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<EngineEvent>();
        // Worker still "open" (we hold ev_tx) — two partials already produced.
        ev_tx
            .send(EngineEvent::Preview {
                rev: 1,
                text: "a".into(),
            })
            .unwrap();
        ev_tx
            .send(EngineEvent::Preview {
                rev: 2,
                text: "ab".into(),
            })
            .unwrap();

        // Same interleave shape as apple_stream_transcription_session: drain
        // events without requiring audio EOF first.
        let mut drained = 0usize;
        while drained < 2 {
            tokio::select! {
                event = ev_rx.recv() => {
                    let Some(event) = event else { break };
                    sink.on_event(&event);
                    drained += 1;
                }
            }
        }
        // Drop worker side only after assert — proves previews did not wait on it.
        drop(ev_tx);
        let got = sink.0.lock().expect("lock").clone();
        assert_eq!(got, vec!["a".to_string(), "ab".to_string()]);
    }

    /// W1-A contract: lexicon correction must land at SEAL time on the Apple
    /// progressive path — before the text becomes committed canvas. The Apple
    /// path used to emit raw SFSpeech text and rely on the stop-path
    /// postprocess, which is a post-commit rewrite (forbidden by the
    /// append-only doctrine).
    #[test]
    fn apple_seal_lexicon_corrects_sealed_final() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "uruchom doker teraz".into(),
                segments: vec![segment("uruchom doker teraz", 0.0, 1.0)],
            }],
            &tx,
            &mut state,
            1.0,
        );
        drop(tx);
        let event = rx.try_recv().expect("sealed final");
        let EngineEvent::UtteranceFinal { text, raw_text, .. } = event else {
            panic!("expected UtteranceFinal, got {event:?}");
        };
        // w2-b: seal ordering is lexicon → Light+. "doker"→"Docker", then
        // sentence capitalisation + terminal period from Light+ left-context.
        assert_eq!(text, "Uruchom Docker teraz.");
        assert_eq!(
            raw_text, "uruchom doker teraz",
            "raw_text must preserve uncorrected engine output for the quality loop"
        );
        assert_eq!(state.sealed_count, 1);
    }

    /// Previews are in-flight presentation, not canvas — they must stay raw so
    /// the correction lands exactly once, at seal time.
    #[test]
    fn apple_seal_lexicon_leaves_previews_raw() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        emit_stream_events(
            vec![LiveStreamEvent::Partial {
                text: "uruchom doker".into(),
                segments: vec![segment("uruchom doker", 0.0, 1.0)],
            }],
            &tx,
            &mut state,
            1.0,
        );
        drop(tx);
        let event = rx.try_recv().expect("preview");
        let EngineEvent::Preview { text, .. } = event else {
            panic!("expected Preview, got {event:?}");
        };
        assert_eq!(text, "uruchom doker");
    }

    /// Utterances that postprocess reduces to nothing must be dropped with an
    /// explicit `FilteredEmpty` signal — never emitted as an empty final.
    #[test]
    fn apple_seal_lexicon_drops_filtered_empty_instead_of_emitting_blank() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                // Trailing-":D" burst: a known ASR artifact that cleanup strips
                // to nothing.
                text: ":D".into(),
                segments: vec![segment(":D", 0.0, 1.0)],
            }],
            &tx,
            &mut state,
            1.0,
        );
        drop(tx);
        let event = rx.try_recv().expect("drop event");
        let EngineEvent::Drop { kind, text, .. } = event else {
            panic!("expected Drop, got {event:?}");
        };
        assert_eq!(kind, DropKind::FilteredEmpty);
        assert_eq!(text, ":D");
        assert!(
            rx.try_recv().is_err(),
            "no final may follow a filtered drop"
        );
        assert_eq!(state.sealed_count, 0);
        assert_eq!(state.filtered_empty_drops, 1);
    }

    /// Partials-only engines never emit a phrase final; the summary fallback is
    /// the seal, so it needs the same correction.
    #[test]
    fn apple_seal_lexicon_corrects_summary_fallback_seal() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        emit_stream_events(
            vec![LiveStreamEvent::Summary {
                text: "zbuduj obraz doker".into(),
                segments: vec![segment("zbuduj obraz doker", 0.0, 2.0)],
                ok: true,
                error: None,
            }],
            &tx,
            &mut state,
            2.0,
        );
        drop(tx);
        let event = rx.try_recv().expect("summary seal");
        let EngineEvent::UtteranceFinal { text, .. } = event else {
            panic!("expected UtteranceFinal, got {event:?}");
        };
        assert_eq!(text, "Zbuduj obraz Docker.");
        assert!(state.open_partial.is_empty());
    }

    /// The stop-path postprocess still runs over committed text. Seal-time
    /// correction is only append-safe because a second application is a no-op.
    #[test]
    fn apple_seal_lexicon_is_idempotent_under_stop_path_postprocess() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "uruchom doker teraz".into(),
                segments: vec![segment("uruchom doker teraz", 0.0, 1.0)],
            }],
            &tx,
            &mut state,
            1.0,
        );
        drop(tx);
        let event = rx.try_recv().expect("sealed final");
        let EngineEvent::UtteranceFinal { text, .. } = event else {
            panic!("expected UtteranceFinal, got {event:?}");
        };
        assert_eq!(
            crate::pipeline::stream_postprocess::apply_lexicon(&text),
            text,
            "stop-path lexicon must not rewrite already-sealed text"
        );
    }

    // ── W2-A · Layer 1 tail-patch on the Apple progressive path ──────────────

    /// Restores an env var to its pre-test value, so a serial env test cannot
    /// leak a phase flag into the rest of the binary.
    struct EnvRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvRestore {
        /// Snapshot `key`'s current value (or absence) for later restore on drop.
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        /// Put the env var back exactly as it was when `capture` ran.
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    /// Collecting sink for Layer 1 / SessionFinalised event assertions.
    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<EngineEvent>>);

    impl EventSink for RecordingSink {
        /// Clone every engine event into the mutex-backed log.
        fn on_event(&self, event: &EngineEvent) {
            self.0.lock().expect("lock").push(event.clone());
        }
    }

    impl RecordingSink {
        /// Snapshot of all events received so far (clone under lock).
        fn events(&self) -> Vec<EngineEvent> {
            self.0.lock().expect("lock").clone()
        }
    }

    fn synthetic_tail_job(utterance_id: u64, outcome: TailPatchOutcome) -> TailPatchJobResult {
        let range = TailSampleRange {
            session: "test-session".to_string(),
            capture_epoch: 0,
            sample_start: 0,
            sample_end: 0,
        };
        TailPatchJobResult {
            utterance_id,
            outcome,
            payload: synthetic_tail_payload(utterance_id, range, Vec::new()),
        }
    }

    /// Wiring contract: a sealed utterance must hand Layer 1 the exact audio
    /// behind it plus the exact committed string `ReplaceRange` offsets are
    /// computed against. Anything else patches canvas from the wrong source.
    #[test]
    fn apple_tail_patch_seal_enqueues_audio_window_for_the_sealed_utterance() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (tp_tx, mut tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
        let mut state = AppleSealState::new_with_tail_patch(TEST_SAMPLE_RATE, tp_tx);
        push_capture(&mut state, 6.0);

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "uruchom doker".into(),
                segments: vec![segment("uruchom doker", 0.5, 2.0)],
            }],
            &tx,
            &mut state,
            6.0,
        );

        let req = tp_rx
            .try_recv()
            .expect("sealed utterance must enqueue a tail-patch request");
        assert_eq!(req.utterance_id, 1);
        assert_eq!(
            req.committed_text, "Uruchom Docker.",
            "Layer 1 must diff against the progressive-sealed text (lexicon → Light+), not raw engine output"
        );
        assert_eq!(
            req.audio.len(),
            2 * TEST_SAMPLE_RATE as usize,
            "window is [previous seal end, end_ts) at session rate"
        );
        assert_eq!(req.provider_request.identity.range.sample_start, 0);
        assert_eq!(req.provider_request.identity.range.sample_end, 32_000);
        assert_eq!(req.provider_request.identity.request_id, req.utterance_id);
        assert_eq!(
            state.tail_patch_awaiting_completion, 1,
            "an accepted request is what the end-of-session closure loop owes a wait to"
        );
    }

    /// SFSpeech may report a word end a few milliseconds past PCM capture.
    /// Ingestion clamps it once onto the integer sample clock; later stages do
    /// not compare the two floating clocks as if they were identical.
    #[test]
    fn apple_segments_map_to_captured_pcm_samples_at_ingestion() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 2.0);

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "zegar pcm".into(),
                segments: vec![segment("zegar pcm", 0.5, 2.002)],
            }],
            &tx,
            &mut state,
            2.0,
        );

        let sealed = &state.progressive.sealed_spans()[0];
        assert_eq!(sealed.range.sample_start, 8_000);
        assert_eq!(sealed.range.sample_end, 32_000);
        assert_eq!(sealed.words[0].range.sample_end, 32_000);
        assert_eq!(sealed.end_secs_millis, 2_002, "legacy adapter unchanged");
    }

    /// The closure loop must wait on outstanding Layer 1 *jobs*, never on the
    /// pending-seal queue. The two diverge the moment a span is held by the
    /// Apple volatile window: no completion can clear that gate, so a loop
    /// watching the seal queue waits for an event that is not coming. That is
    /// what parked the stop path for the full timeout on 2026-08-12.
    #[test]
    fn tail_patch_closure_counter_tracks_jobs_not_pending_seals() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (tp_tx, mut tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
        let mut state = AppleSealState::new_with_tail_patch(TEST_SAMPLE_RATE, tp_tx);
        push_capture(&mut state, 6.0);

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "uruchom doker".into(),
                segments: vec![segment("uruchom doker", 0.5, 2.0)],
            }],
            &tx,
            &mut state,
            6.0,
        );
        let req = tp_rx
            .try_recv()
            .expect("sealed utterance enqueues a request");
        assert_eq!(state.tail_patch_awaiting_completion, 1);

        // Close the job on a clock that is still inside the span's volatile
        // window — the exact shape the old exit condition could not express.
        state.complete_whisper_window(
            &tx,
            TailPatchCompletion {
                utterance_id: req.utterance_id,
                covered_through_secs: req.covered_through_secs,
                outcome: TailPatchOutcome::skipped(
                    crate::stt::tail_patcher::SkipReasonCode::EmptyRetranscription,
                    "no change",
                ),
                payload: None,
            },
            2.1,
        );

        assert_eq!(
            state.tail_patch_awaiting_completion, 0,
            "every job reported back — the stop path owes no further wait"
        );
        assert!(
            !state.progressive.pending_spans().is_empty(),
            "yet a span is still pending: waiting on this queue would hang on nothing"
        );
    }

    /// F3 carry-over: a boundary that does not address retained audio already
    /// counts as unresolved. It must also never reach Whisper — patching from
    /// the wrong span is worse than not patching.
    #[test]
    fn apple_tail_patch_unresolved_window_enqueues_nothing() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (tp_tx, mut tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
        let mut state = AppleSealState::new_with_tail_patch(TEST_SAMPLE_RATE, tp_tx);
        push_capture(&mut state, 2.0);

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "zdanie z przyszlosci".into(),
                segments: vec![segment("zdanie z przyszlosci", 8.0, 9.0)],
            }],
            &tx,
            &mut state,
            2.0,
        );

        assert_eq!(state.sealed_count, 1, "the text still seals");
        assert_eq!(state.unresolved_windows, 1);
        assert!(
            tp_rx.try_recv().is_err(),
            "an unresolved window must not be handed to Layer 1"
        );
    }

    /// Layered off (default): the seal path carries no Layer 1 wire at all, so
    /// zero jobs can be scheduled from it.
    #[test]
    fn apple_tail_patch_off_by_default_enqueues_no_jobs() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 4.0);

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "uruchom doker".into(),
                segments: vec![segment("uruchom doker", 0.5, 2.0)],
            }],
            &tx,
            &mut state,
            4.0,
        );

        assert!(
            state.tail_patch.is_none(),
            "no wire exists when layered is off"
        );
        assert_eq!(state.sealed_count, 1);
        assert_eq!(state.tail_patch_backpressure_drops, 0);
    }

    /// F1: the seal path runs on the worker thread that also forwards PCM into
    /// the bridge. It must never block on the patch queue — a full queue drops
    /// and counts, capture keeps flowing.
    #[test]
    fn apple_tail_patch_backpressure_drops_instead_of_stalling_capture() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (tp_tx, _tp_rx) = mpsc::channel::<TailPatchRequest>(1);
        let mut state = AppleSealState::new_with_tail_patch(TEST_SAMPLE_RATE, tp_tx);
        push_capture(&mut state, 10.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "pierwsze zdanie".into(),
                    segments: vec![segment("pierwsze zdanie", 0.5, 2.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "drugie zdanie".into(),
                    segments: vec![segment("drugie zdanie", 2.5, 4.0)],
                },
            ],
            &tx,
            &mut state,
            10.0,
        );

        assert_eq!(state.sealed_count, 2, "seals never wait on the patch queue");
        assert_eq!(state.tail_patch_backpressure_drops, 1);
    }

    /// Acceptance arm: an induced gap (Layer 0 committed a shorter span than
    /// the audio actually carried) emits exactly one bounded
    /// `ReplaceRange{TailPatch}` and increments the count that
    /// `SessionFinalised.layer_summary` reports.
    #[tokio::test]
    async fn apple_tail_patch_induced_gap_emits_replace_range_and_counts_into_summary() {
        let sink = RecordingSink::default();
        let mut lane = AppleTailPatchLane::new(TEST_SAMPLE_RATE, None);

        // Layer 0 sealed the phrase without its tail word; Whisper heard it.
        let outcome = compute_tail_patch(
            "ala ma kota w domu",
            "ala ma kota w domu swoim",
            1,
            &TailPatchConfig::default(),
        );
        lane.push_job(Box::pin(async move { Ok(synthetic_tail_job(1, outcome)) }));

        let result = lane.next().await.expect("one job in flight");
        lane.complete(&sink, result);

        assert_eq!(lane.replacements(), 1);
        let replaces: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    EngineEvent::ReplaceRange {
                        source: LayerSource::TailPatch,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(replaces.len(), 1, "one bounded patch, never a full rewrite");

        emit_session_finalised(&sink, "session".to_string(), lane.replacements());
        let finalised = sink
            .events()
            .into_iter()
            .find_map(|e| match e {
                EngineEvent::SessionFinalised { layer_summary, .. } => Some(layer_summary),
                _ => None,
            })
            .expect("SessionFinalised");
        assert_eq!(finalised.tail_patch_replacements, 1);
    }

    /// F2: the shared `TailPatchConfig` threshold still owns the decision — a
    /// re-transcription that diverges wholesale is skipped, not applied.
    /// Thresholds are untouched by this cut.
    #[tokio::test]
    async fn apple_tail_patch_divergent_retranscription_is_skipped_not_applied() {
        let sink = RecordingSink::default();
        let mut lane = AppleTailPatchLane::new(TEST_SAMPLE_RATE, None);

        let outcome = compute_tail_patch(
            "ala ma kota w domu",
            "zupelnie inne zdanie bez zwiazku calkiem",
            1,
            &TailPatchConfig::default(),
        );
        assert!(
            matches!(outcome, TailPatchOutcome::Skipped { .. }),
            "shared threshold must reject a wholesale divergence"
        );
        lane.push_job(Box::pin(async move { Ok(synthetic_tail_job(1, outcome)) }));

        let result = lane.next().await.expect("one job in flight");
        lane.complete(&sink, result);

        assert_eq!(lane.replacements(), 0);
        assert_eq!(
            lane.skipped(),
            1,
            "a skipped job must count toward the starvation receipt"
        );
        assert!(
            sink.events().is_empty(),
            "a skipped patch must not touch committed canvas"
        );
    }

    /// The env gate is the only control: default (unset) arms the lane — the
    /// live tail patch is a core element of the triangulation, not an opt-in
    /// (operator directive 2026-08-09). Explicit `off` is the one way out.
    #[test]
    #[serial]
    fn apple_tail_patch_lane_is_wired_by_default_and_off_disarms() {
        let _restore = EnvRestore::capture(LAYERED_TRANSCRIPTION_ENV);

        unsafe { std::env::remove_var(LAYERED_TRANSCRIPTION_ENV) };
        assert!(tail_patch_enabled(), "default arms the live tail patch");

        unsafe { std::env::set_var(LAYERED_TRANSCRIPTION_ENV, "off") };
        assert!(!tail_patch_enabled(), "explicit off disarms");

        unsafe { std::env::set_var(LAYERED_TRANSCRIPTION_ENV, "phase1") };
        assert!(tail_patch_enabled(), "phase1 arms Layer 1");
    }

    /// Bridge stdout lines with multiple `final` events parse as phrase seals.
    #[test]
    fn parse_lines_feed_multi_seal_count() {
        let lines = [
            r#"{"event":"final","text":"a"}"#,
            r#"{"event":"final","text":"b"}"#,
            r#"{"event":"final","text":"c"}"#,
        ];
        let events: Vec<_> = lines
            .iter()
            .filter_map(|l| parse_stream_stdout_line(l))
            .collect();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, LiveStreamEvent::PhraseFinal { .. }))
                .count(),
            3
        );
    }

    // ── w1-b utterance_drop: shared_opener_restart_suppresses_freeze ────────

    /// One checked-in vector source is consumed by this Rust mirror and the
    /// Swift bridge self-test. The measured 40→20 non-prefix collapse is the
    /// RED discriminator: threshold-only restart detection currently loses it.
    #[test]
    fn fleet_red_retention_missed_collapse_40_to_20() {
        let vectors = include_str!("../../../tests/fixtures/phrase_restart_vectors.tsv");
        let required_ids = [
            "measured_restart_47_to_12",
            "measured_revision_95_to_79",
            "missed_collapse_40_to_20",
            "shared_opener_sentence_restart",
            "shared_opener_spoken_variant",
        ];
        let mut seen_ids = std::collections::BTreeSet::new();

        for line in vectors.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 4, "malformed phrase restart vector: {line}");
            seen_ids.insert(fields[0]);
            let expected = fields[1]
                .parse::<bool>()
                .expect("expected_freeze must be true or false");
            let actual = phrase_restart_should_freeze_prior(fields[2], fields[3]);
            if fields[0] == "missed_collapse_40_to_20" {
                assert_eq!(fields[2].chars().count(), 40);
                assert_eq!(fields[3].chars().count(), 20);
            }
            assert_eq!(
                actual,
                expected,
                "phrase restart vector {} diverged: prev_chars={} next_chars={}",
                fields[0],
                fields[2].chars().count(),
                fields[3].chars().count()
            );
        }

        for required_id in required_ids {
            assert!(
                seen_ids.contains(required_id),
                "required phrase restart vector missing: {required_id}"
            );
        }
    }

    /// Measured three-way pattern: after a long open partial, SFSpeech collapses
    /// onto the next sentence's shared opener (`Zdanie`). That collapse MUST
    /// freeze the prior utterance — the old rule did not, and s6/s8/s10 vanished.
    #[test]
    fn utterance_drop_shared_opener_restart_freezes_prior_sentence() {
        let s6 = "Zdanie szóste spokojnie po stresie wracam do normalnego tempa i mówię wyraźnie.";
        assert!(
            phrase_restart_should_freeze_prior(s6, "Zdanie"),
            "collapse onto the next sentence's shared opener must freeze s6"
        );
        assert!(
            phrase_restart_should_freeze_prior(s6, "Zdanie siódme"),
            "collapse onto a non-prefix next-sentence head must freeze s6"
        );
        assert!(
            phrase_restart_should_freeze_prior(s6, "Zadanie"),
            "Zadanie opener (spoken variant) must freeze too"
        );
    }

    /// Revisions and rewinds must retain the prior text; only a forward
    /// extension that contains the full prior hypothesis may replace it.
    #[test]
    fn utterance_drop_revision_and_rewind_retain_prior() {
        // 95 → 79 char mid-reword is classified as a revision, but still
        // freezes because otherwise its removed span has no retained copy.
        let prev = format!("{}MIDDLE{}", "x".repeat(50), "y".repeat(39));
        let next = format!("{}REVISE{}", "x".repeat(50), "y".repeat(23));
        assert_eq!(prev.len(), 95);
        assert_eq!(next.len(), 79);
        assert!(
            phrase_restart_should_freeze_prior(&prev, &next),
            "revision must retain the prior hypothesis"
        );
        // Forward growth contains the complete prior hypothesis.
        assert!(!phrase_restart_should_freeze_prior(
            "Zdanie",
            "Zdanie szóste spokojnie"
        ));
        // Same-phrase rewind is not safe unless the prior copy is retained.
        let long = "Hello world this is a long phrase that continues for a while more text here";
        let rewind: String = long.chars().take(40).collect();
        assert!(
            phrase_restart_should_freeze_prior(long, &rewind),
            "substantial true-prefix rewind must retain its removed suffix"
        );
        assert!(phrase_restart_should_freeze_prior(long, ""));
        assert!(!phrase_restart_should_freeze_prior("", "new phrase"));
        assert!(!phrase_restart_should_freeze_prior(
            "middle retained",
            "new prefix middle retained and suffix"
        ));
    }

    /// End-to-end at the adjudication layer: a partial sequence that used to
    /// drop the post-stressor sentence now seals it as UtteranceFinal before
    /// the restart partial lands as Preview.
    #[test]
    fn utterance_drop_emit_seals_prior_on_shared_opener_partial_restart() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        let s5 = "Zdanie piąte, szybko bez pauz. Teraz mówię bardzo szybko, bez żadnej przerwy, \
                  żeby sprawdzić czy silnik nadąża za tempem, którego normalnie unika w \
                  codziennym dyktowaniu.";
        let s6 = "Zdanie szóste spokojnie po stresie wracam do normalnego tempa i mówię wyraźnie.";
        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: s5.to_string(),
                    segments: vec![segment(s5, 0.0, 5.0)],
                },
                // Stressor phrase seals cleanly (isFinal or prior freeze).
                LiveStreamEvent::PhraseFinal {
                    text: s5.to_string(),
                    segments: vec![segment(s5, 0.0, 5.0)],
                },
                // Post-stressor sentence builds as open partial…
                LiveStreamEvent::Partial {
                    text: s6.to_string(),
                    segments: vec![segment(s6, 5.0, 10.0)],
                },
                // …then SFSpeech restarts onto the next opener without isFinal.
                // Old rule overwrote s6; new rule freezes it first.
                LiveStreamEvent::Partial {
                    text: "Zdanie".to_string(),
                    segments: vec![segment("Zdanie", 10.0, 10.5)],
                },
                LiveStreamEvent::Partial {
                    text: "Zdanie siódme Overlap cztery angielskie terminy w polskim".to_string(),
                    segments: vec![segment(
                        "Zdanie siódme Overlap cztery angielskie terminy w polskim",
                        10.0,
                        15.0,
                    )],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "Zdanie siódme Overlap cztery angielskie terminy w polskim".to_string(),
                    segments: vec![segment(
                        "Zdanie siódme Overlap cztery angielskie terminy w polskim",
                        10.0,
                        15.0,
                    )],
                },
            ],
            &tx,
            &mut state,
            30.0,
        );
        drop(tx);
        let mut finals = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let EngineEvent::UtteranceFinal { text, .. } = e {
                finals.push(text);
            }
        }
        assert!(
            finals
                .iter()
                .any(|t| t.contains("szóste") || t.contains("szost")),
            "post-stressor s6 must be committed, got finals: {finals:?}"
        );
        assert!(
            finals
                .iter()
                .any(|t| t.contains("siódme") || t.contains("siodm") || t.contains("Overlap")),
            "s7 must still seal, got finals: {finals:?}"
        );
        assert!(
            state.sealed_count >= 3,
            "s5 + frozen s6 + s7 → at least 3 seals, got {}",
            state.sealed_count
        );
    }
}
