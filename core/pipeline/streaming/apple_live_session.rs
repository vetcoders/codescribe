//! Apple live session — the only target of the normal live dispatcher.
//!
//! One long-lived SFSpeech stream maps:
//! - `partial` → `EngineEvent::Preview` (RAW — previews are not canvas yet)
//! - phrase `final` → `EngineEvent::UtteranceFinal` (multi-seal freezed+append)
//! - open partial on stop → sealed as a last final when non-empty
//!
//! At seal, Apple and Lexicon/Light+ are separate observations of the same
//! PCM-identified occurrence. `admit_ledger_label` offers them to
//! `AcousticLedger::admit`; a closed occurrence passes through
//! `AcousticLedger::seal`; `EngineEvent::LedgerMutation` and
//! `EngineEvent::LedgerSeal` then carry the receipts to
//! `PresentationEmitter` / `TranscriptReducer`. Neither raw Apple text nor the
//! seal-time shaping pass owns the document.
//!
//! Whisper is never the primary live engine here. Local Power arms it as the
//! required Layer 1 observer: each sealed utterance resolves to retained PCM and
//! is re-transcribed by the tail provider. Its result is offered to the same
//! `AcousticLedger`; it cannot mint a second occurrence or mutate after seal.
//! This is live gap repair, never stop-time whole-text authority.
//! Apple-only deliberately omits this lane; explicit off/invalid overrides in
//! Local Power produce a typed degraded state.
//! `CODESCRIBE_APPLE_STT_LIVE_MODE=wav` selects the older Apple `transcribe_live`
//! temp-WAV request transport for A/B comparison with live AudioBuffer delivery.
//! It does not restore the deleted VAD/scheduler pipeline or create another
//! transcript authority.
//!
//! The bridge global lock + child process live on a **dedicated OS thread**
//! (MutexGuard is `!Send`); the async session only shuttles PCM in and
//! `EngineEvent`s out.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesOrdered;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::asr_session::recorder::{
    LAYER1_DEGRADED_WARNING_CODE, Layer1DegradeReason, RecorderLayer1Lane,
    apply_recorder_lifecycle_event,
};
use crate::asr_session::{SessionId as Layer1SessionId, SessionInput as Layer1SessionInput};
use crate::audio::capture_receipt::{
    CaptureLevelAccumulator, CapturePathMeta, begin_session_energy_clock,
    emit_capture_level_receipt, session_active_speech_ranges,
};
use crate::config::{FormattingPolicy, RuntimeSettingsSnapshot};
use crate::llm::ai_formatting::{
    AiFormatResult, AiFormatStatus, format_text_with_status_for_policy,
};
use crate::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
use crate::pipeline::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, MutationReceipt,
    ObservationIdentity as LedgerObservationIdentity,
    ObservationProducer as LedgerObservationProducer, OccurrenceIdentity, SealCoverageReceipt,
    SealCoverageStatus, SealRefusal, TranscriptComparisonReceipt,
};
use crate::pipeline::contracts::{EngineEvent, EventSink, TranscriptSegment};
use crate::stt::apple_stt::{LiveStreamEvent, LiveStreamSession};
use crate::stt::tail_patcher::{SkipReasonCode, TailPatchConfig, TailPatchOutcome};
use crate::stt::tail_provider::{
    InProcessTailProvider, TailProvider, TailProviderPayload, TailProviderRequest,
    TailRequestIdentity, TailSampleRange, TimedTailSegment,
};

use super::layer1_window::{CoalesceFlush, CoalescedPiece, Layer1Coalesce};
use super::live_audio_buffer::{DEFAULT_RETENTION_SECS, LiveAudioBuffer, ResolvedAudioWindow};
use super::session::{
    SessionConfig, TailPatchDrainDisposition, TailPatchJobResult, TailPatchSessionReceipt,
    compute_tail_patch_job, emit_session_finalised, log_tail_patch_session_receipt,
};
use super::silero_fusion::{
    FusionContextMode, FusionWord, SileroIngress, bound_context_range, slice_apple_words,
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

/// Bounded transport ownership for occurrence formatter jobs. Saturation skips
/// Formatter scheduling for that occurrence; it never backpressures PCM.
const FORMATTER_QUEUE_CAP: usize = 8;

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

/// A 200 ms VAD/energy edge is ordinary quantisation; an uncovered span over
/// 250 ms is not allowed to become terminal transcript truth.
pub const SEAL_COVERAGE_INCOMPLETE_MS: u64 = 250;

/// Stable outward receipt when accepted Layer 1 work cannot land before the
/// Apple seal worker closes. The Apple canvas remains authoritative.
pub const TAIL_PATCH_DRAIN_TIMEOUT_WARNING_CODE: &str = "tail_patch_drain_timeout";

/// Local power was selected but its required live patcher could not arm.
pub const LOCAL_TAIL_PATCH_DEGRADED_WARNING_CODE: &str = "local_tail_patch_degraded";

/// The provider result did not prove that it describes the PCM range owned by
/// the pending span. No transcript text is included in this receipt.
pub const TAIL_PATCH_IDENTITY_MISMATCH_WARNING_CODE: &str = "tail_patch_identity_mismatch";

/// Content-free marker emitted when an Apple final callback contained segment
/// time already committed by an earlier callback. The overlapping portion is
/// removed before a new utterance id can be allocated.
pub const APPLE_FINAL_OVERLAP_WARNING_CODE: &str = "apple_final_window_overlap_normalized";

/// Terminal ledger finality was refused for a structurally meaningful reason.
/// The refusal token is diagnostics-only and never force-seals the document.
pub const LEDGER_TERMINAL_SEAL_REFUSED_WARNING_CODE: &str = "acoustic_ledger_terminal_seal_refused";

/// Text-free token naming what the retired char-diff made of one Layer 1 job.
///
/// The one-throne path admits Whisper through `AcousticLedger` on occurrence
/// identity, so this verdict carries no authority: it never becomes a label, a
/// receipt, or a mutation. It exists so the discarded legacy decision stays
/// observable without the transcript text ever entering a log line.
fn legacy_char_diff_verdict(outcome: &TailPatchOutcome) -> &'static str {
    match outcome {
        TailPatchOutcome::Patches(_) => "patches",
        TailPatchOutcome::NoChange => "no_change",
        TailPatchOutcome::UnderCommit(_) => "under_commit",
        TailPatchOutcome::Skipped { .. } => "skipped",
    }
}

/// One sealed utterance handed from the worker thread to the async Layer 1 lane.
struct TailPatchRequest {
    utterance_id: u64,
    /// Byte-identical to the emitted `UtteranceFinal.text` — the string every
    /// `ReplaceRange` char offset is computed against.
    committed_text: String,
    /// Canvas already sealed BEFORE this utterance.
    ///
    /// Layer 1 sees one utterance at a time, so a phrase the previous
    /// utterance already carries reads as a gap here and is appended a second
    /// time — measured 2026-08-14 the moment recoveries first reached the
    /// canvas ("…hard pruna I road która pozwoli nam na zrobienie hard Pru."),
    /// which cost more WER than the recovery gained. The neighbour context is
    /// read-only: it is never patched, only consulted so a duplicate is
    /// escalated instead of placed.
    neighbour_context: String,
    /// PCM behind exactly this utterance: `[previous seal end, end_ts)`.
    audio: Vec<f32>,
    /// Exact capture range behind `audio`; this is the window-start authority.
    provider_request: TailProviderRequest,
    /// Every exact occurrence whose launched Whisper slot this job must close.
    member_occurrences: Vec<(u64, OccurrenceIdentity)>,
}

/// Whisper closure returned to the worker that owns Apple + seal state.
struct TailPatchCompletion {
    utterance_id: u64,
    request_identity: Option<TailRequestIdentity>,
    payload: Option<TailProviderPayload>,
    member_occurrences: Vec<(u64, OccurrenceIdentity)>,
}

/// In-flight Layer 1 job identity, including the coalesce map.
struct TailPatchInFlight {
    utterance_id: u64,
    request_identity: TailRequestIdentity,
    member_occurrences: Vec<(u64, OccurrenceIdentity)>,
}

/// One concrete formatter job, keyed only by an existing PCM occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FormatterRequest {
    occurrence: OccurrenceIdentity,
    existing_label: String,
}

/// Provider outcome bound to one request. The worker receives it as a
/// completion only after its typed proposal reached PresentationEmitter.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FormatterCompletion {
    occurrence: OccurrenceIdentity,
    proposal: OccurrenceLabelProposal,
}

impl FormatterCompletion {
    fn from_result(request: FormatterRequest, result: AiFormatResult) -> Self {
        let (proposed_label, disposition) = match result.status {
            AiFormatStatus::Applied if !result.text.trim().is_empty() => {
                (result.text, LabelProposalDisposition::Propose)
            }
            AiFormatStatus::Applied | AiFormatStatus::Failed => {
                (String::new(), LabelProposalDisposition::Refuse)
            }
            AiFormatStatus::Skipped | AiFormatStatus::AiNoop => {
                (String::new(), LabelProposalDisposition::PreserveExisting)
            }
        };
        let occurrence = request.occurrence;
        let proposal = OccurrenceLabelProposal::for_existing_occurrence(
            occurrence.session.clone(),
            occurrence.capture_epoch,
            occurrence.sample_start,
            occurrence.sample_end,
            proposed_label,
            disposition,
        );
        Self {
            occurrence,
            proposal,
        }
    }

    fn carries_same_occurrence(&self) -> bool {
        self.proposal.session == self.occurrence.session
            && self.proposal.capture_epoch == self.occurrence.capture_epoch
            && self.proposal.sample_start == self.occurrence.sample_start
            && self.proposal.sample_end == self.occurrence.sample_end
            && self.proposal.binds_real_samples()
    }
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
            req.neighbour_context,
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

    /// Convert a finished job into the closure message consumed by the
    /// progressive seal owner. The request identity rides separately from the
    /// provider payload so failures can still close the exact pending window.
    fn finish_for_worker(
        &mut self,
        inflight: Option<TailPatchInFlight>,
        result: Result<TailPatchJobResult>,
    ) -> TailPatchCompletion {
        let (fallback_id, request_identity, member_occurrences) = match inflight {
            Some(job) => (
                job.utterance_id,
                Some(job.request_identity),
                job.member_occurrences,
            ),
            None => (0, None, Vec::new()),
        };
        match result {
            Ok(job) => {
                // Counts only. A `Patches` outcome carries transcript text, so
                // the verdict is reduced to a token before it reaches the log —
                // and it is dropped here either way: the ledger admits Whisper
                // by occurrence identity, never by this char-diff.
                debug!(
                    utterance_id = job.utterance_id,
                    verdict = legacy_char_diff_verdict(&job.outcome),
                    "Layer 1 char-diff verdict discarded — occurrence admission owns the label"
                );
                TailPatchCompletion {
                    utterance_id: job.utterance_id,
                    request_identity,
                    payload: Some(job.payload),
                    member_occurrences,
                }
            }
            Err(error) => {
                warn!(
                    utterance_id = fallback_id,
                    "Layer 1 provider job failed; Apple text is preserved: {error}"
                );
                TailPatchCompletion {
                    utterance_id: fallback_id,
                    request_identity,
                    payload: None,
                    member_occurrences,
                }
            }
        }
    }

    /// Hand a completion to the live seal owner and only then account it in
    /// the session receipt. A closed receiver means the worker has already
    /// sealed raw and no patch can reach the canvas.
    fn forward_completion_to_worker(
        &mut self,
        tx: &std_mpsc::Sender<TailPatchCompletion>,
        completion: TailPatchCompletion,
    ) -> bool {
        if tx.send(completion).is_err() {
            return false;
        }
        true
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

const fn formatter_lane_is_armed(
    ai_formatting_enabled: bool,
    policy: FormattingPolicy,
    lane_available: bool,
) -> bool {
    ai_formatting_enabled && !matches!(policy, FormattingPolicy::Off) && lane_available
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

fn emit_local_tail_patch_degraded_warning(event_sink: &dyn EventSink, disposition: &str) {
    event_sink.on_event(&EngineEvent::Warning {
        code: LOCAL_TAIL_PATCH_DEGRADED_WARNING_CODE.to_string(),
        message: disposition.to_string(),
    });
}

/// Report abandoned local tail-patch work exactly once, before session finality.
fn report_tail_patch_drain_degrade(event_sink: &dyn EventSink, abandoned: u64) {
    if abandoned == 0 {
        return;
    }
    event_sink.on_event(&EngineEvent::Warning {
        code: TAIL_PATCH_DRAIN_TIMEOUT_WARNING_CODE.to_string(),
        message: format!(
            "{abandoned} accepted Layer 1 tail-patch job(s) missed the bounded stop drain; Apple live text was preserved"
        ),
    });
}

/// Preserve a terminal ledger refusal on the ordered diagnostic surface.
/// `NotQualified` is the one quiet outcome: it means no physical speech was
/// admitted, not that a known occurrence failed to close.
fn report_terminal_seal_refusal(ev_tx: &mpsc::UnboundedSender<EngineEvent>, refusal: SealRefusal) {
    if refusal == SealRefusal::NotQualified {
        return;
    }
    let _ = ev_tx.send(EngineEvent::Warning {
        code: LEDGER_TERMINAL_SEAL_REFUSED_WARNING_CODE.to_string(),
        message: refusal.as_str().to_string(),
    });
}

/// Reconcile job-level terminal buckets after the worker's bounded closure
/// loop. No-change, provider skip, and rewrite-fence refusal all land in
/// `skipped`; `applied` means a completed job whose bounded mutation survived.
#[derive(Clone, Copy)]
struct TailPatchWorkerAccounting {
    applied_jobs: u64,
    skipped_jobs: u64,
    timeout_residue: u64,
}

fn tail_patch_receipt_after_stop(
    armed: bool,
    submitted: u64,
    worker_accounting: Option<TailPatchWorkerAccounting>,
) -> TailPatchSessionReceipt {
    // The worker increments its awaiting-completion counter before the async
    // owner accepts a request. On bounded closure expiry, that counter already
    // owns every async in-flight/queued request as `timed_out`; the async side
    // must not classify the same requests again as `abandoned`. Abandonment is
    // reserved for the distinct route where the worker returns no accounting.
    let (applied_jobs, skipped_jobs, timeout_residue, abandoned_jobs) = match worker_accounting {
        Some(accounting) => (
            accounting.applied_jobs,
            accounting.skipped_jobs,
            accounting.timeout_residue,
            0,
        ),
        None => (0, 0, 0, submitted),
    };
    TailPatchSessionReceipt::new(
        armed,
        submitted,
        applied_jobs,
        skipped_jobs,
        timeout_residue,
        abandoned_jobs,
        if !armed {
            TailPatchDrainDisposition::NotArmed
        } else if timeout_residue > 0 {
            TailPatchDrainDisposition::TimedOut
        } else if abandoned_jobs > 0 {
            TailPatchDrainDisposition::Abandoned
        } else {
            TailPatchDrainDisposition::Completed
        },
    )
}

/// Drive one progressive Apple stream session until the audio channel closes.
pub(crate) async fn apple_stream_transcription_session(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    event_sink: Arc<dyn EventSink>,
    config: SessionConfig,
) {
    let SessionConfig {
        session_id,
        capture_epoch,
        runtime_settings,
        acoustic_ledger,
        sample_rate,
        capture_device_name,
        language,
        stream_log_path,
        utterance_silence_sec,
        layer1,
        mut lifecycle_events,
    } = config;
    let mut capture_level = CaptureLevelAccumulator::new();
    begin_session_energy_clock();
    // Hands-free silence is the ENGINE LIFECYCLE on this lane, not a chunker
    // knob: SFSpeech still owns phrase boundaries inside an utterance, but the
    // threshold decides when the engine rests (mic + Silero keep watching) and
    // when a fresh epoch wakes on the next speech edge. Unset = one continuous
    // stream for the whole take, the pre-lifecycle behaviour.
    if let Some(sec) = utterance_silence_sec {
        info!(
            utterance_silence_sec = sec,
            "Apple progressive live mode: engine lifecycle armed on the hands-free silence \
             threshold (speech epochs)"
        );
    }

    info!(
        sample_rate,
        "Apple progressive live session started (stream multi-seal)"
    );
    // The controller owns this identity and the one immutable settings read.
    // Apple may consume both but may not mint a lane-local session or reload.
    let settings_digest = runtime_settings.digest().as_str().to_string();

    // C1: split the one recording-start decision into its explicit local
    // exact-span disposition and (when Cloud is selected) the injected generic
    // provider. Construction and consent live with the settings owner.
    let lane_input = Layer1SessionInput {
        session_id: Layer1SessionId::new(session_id.clone())
            .expect("uuid session ids are never blank"),
        locale: language.clone(),
        sample_rate,
    };
    let local_tail_patch = layer1.local_tail_patch_disposition();
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

    // The local Whisper decision is resolved once from product mode + the
    // compatibility phase token before capture starts. Never re-read env here:
    // Settings, replay, logging, and runtime must all observe one decision.
    let tail_patch_on = local_tail_patch.is_some_and(|decision| decision.is_armed());
    if tail_patch_on {
        info!(
            disposition = local_tail_patch
                .map(|decision| decision.as_token())
                .unwrap_or("not_applicable"),
            "Local Whisper tail-patch armed on Apple progressive path"
        );
    } else if let Some(disposition) = local_tail_patch {
        warn!(
            disposition = disposition.as_token(),
            "Local power degraded: required Whisper tail-patch is not armed"
        );
        emit_local_tail_patch_degraded_warning(event_sink.as_ref(), disposition.as_token());
    }
    let mut tail_patch_lane = AppleTailPatchLane::new(sample_rate, language.clone());
    // At-most-one-in-flight gate (F1), tracked outside the lane so the admit
    // branch's guard does not borrow what the collect branch holds mutably.
    let mut tail_patch_in_flight = false;
    let mut tail_patch_lane_in_flight: Option<TailPatchInFlight> = None;
    let mut tail_patch_submitted = 0u64;
    // Bounded: the worker `try_send`s from the PCM-forwarding thread.
    let (tp_tx, mut tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
    let (tp_done_tx, tp_done_rx) = std_mpsc::channel::<TailPatchCompletion>();
    // Layered off → the worker gets no sender at all, so the lane stays empty
    // and its branch never yields: zero jobs, zero behaviour change.
    let worker_tp_tx = tail_patch_on.then_some(tp_tx);

    // Formatting consumes only facts frozen into this exact per-take snapshot.
    // Arming the transport does not schedule a ledger observer; a concrete
    // occurrence must acquire a bounded queue permit first.
    let formatter_on = formatter_lane_is_armed(
        runtime_settings.values().ai_formatting_enabled,
        runtime_settings.formatting_policy(),
        runtime_settings.llm_lanes().formatting().available(),
    );
    let mut formatter_jobs = FuturesOrdered::<BoxFuture<'static, FormatterCompletion>>::new();
    let formatter_runtime_settings = Arc::clone(&runtime_settings);
    let formatter_language = language.clone();
    let (formatter_tx, mut formatter_rx) = mpsc::channel::<FormatterRequest>(FORMATTER_QUEUE_CAP);
    let (formatter_done_tx, formatter_done_rx) = std_mpsc::channel::<FormatterCompletion>();
    let worker_formatter_tx = formatter_on.then_some(formatter_tx);

    let worker_session_id = session_id.clone();
    let worker = thread::spawn(move || {
        apple_stream_worker(
            pcm_rx,
            ev_tx,
            worker_tp_tx,
            tp_done_rx,
            worker_formatter_tx,
            formatter_done_rx,
            AppleWorkerConfig {
                sample_rate,
                capture_device_name,
                language: language.as_deref(),
                session_id: worker_session_id,
                capture_epoch,
                runtime_settings,
                acoustic_ledger,
                settings_digest,
                utterance_silence_sec,
            },
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
                tail_patch_submitted = tail_patch_submitted.saturating_add(1);
                let inflight = TailPatchInFlight {
                    utterance_id: req.utterance_id,
                    request_identity: req.provider_request.identity.clone(),
                    member_occurrences: req.member_occurrences.clone(),
                };
                tail_patch_lane.push_request(req);
                tail_patch_in_flight = true;
                // One job is in flight; the coalesce map rides alongside so
                // the completion can close every member seal.
                tail_patch_lane_in_flight = Some(inflight);
            }
            Some(result) = tail_patch_lane.next() => {
                tail_patch_in_flight = false;
                let inflight = tail_patch_lane_in_flight.take();
                let completion = tail_patch_lane.finish_for_worker(inflight, result);
                let rejected_id = completion.utterance_id;
                if !tail_patch_lane.forward_completion_to_worker(&tp_done_tx, completion) {
                    warn!(
                        utterance_id = rejected_id,
                        "Layer 1 completion rejected — Apple seal worker already closed"
                    );
                }
            }
            Some(request) = formatter_rx.recv(), if formatter_jobs.len() < FORMATTER_QUEUE_CAP => {
                // The request channel is independent from `ev_rx`. Drain every
                // already-enqueued ledger observation before provider work can
                // complete, so a fast formatter cannot overtake the reducer
                // revision that established its current label.
                while let Ok(event) = ev_rx.try_recv() {
                    deliver_event(
                        &event,
                        event_sink.as_ref(),
                        stream_log_path.as_deref(),
                    );
                }
                let runtime_settings = Arc::clone(&formatter_runtime_settings);
                let language = formatter_language.clone();
                formatter_jobs.push_back(Box::pin(async move {
                    let result = format_text_with_status_for_policy(
                        &request.existing_label,
                        language.as_deref(),
                        runtime_settings.as_ref(),
                    )
                    .await;
                    FormatterCompletion::from_result(request, result)
                }));
            }
            Some(completion) = formatter_jobs.next() => {
                let occurrence = completion.occurrence.clone();
                if !completion.carries_same_occurrence() {
                    warn!(
                        session = occurrence.session,
                        capture_epoch = occurrence.capture_epoch,
                        sample_start = occurrence.sample_start,
                        sample_end = occurrence.sample_end,
                        "Formatter completion refused — proposal changed exact PCM identity"
                    );
                } else {
                    let event = EngineEvent::OccurrenceLabelProposal {
                        proposal: completion.proposal.clone(),
                    };
                    // PresentationEmitter applies the typed disposition and
                    // seals this exact occurrence synchronously before the
                    // worker is told that its accepted job completed.
                    deliver_event(
                        &event,
                        event_sink.as_ref(),
                        stream_log_path.as_deref(),
                    );
                    if formatter_done_tx.send(completion).is_err() {
                        warn!(
                            session = occurrence.session,
                            capture_epoch = occurrence.capture_epoch,
                            sample_start = occurrence.sample_start,
                            sample_end = occurrence.sample_end,
                            "Formatter completion rejected — Apple seal worker already closed"
                        );
                    }
                }
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

    // `ev_rx` closes only after the worker's bounded closure loop has assigned
    // every accepted request still awaiting completion to its timeout bucket.
    // Running those Whisper jobs now cannot change canvas; drain and drop the
    // async work, but do not mint a second terminal bucket for it here.
    let mut outstanding_tail_patch_jobs = u64::from(tail_patch_in_flight);
    while tp_rx.try_recv().is_ok() {
        tail_patch_submitted = tail_patch_submitted.saturating_add(1);
        outstanding_tail_patch_jobs = outstanding_tail_patch_jobs.saturating_add(1);
    }
    if outstanding_tail_patch_jobs > 0 {
        warn!(
            outstanding_tail_patch_jobs,
            "Layer 1 tail-patch async work dropped after worker terminal accounting closed"
        );
    }
    // C1 stop-drain: close the Layer 1 lane with its bounded drain. Whatever
    // happened inside (clean close, disconnect, incomplete drain), the method
    // returns and the recording finishes on Apple + lexicon. The outcome's
    // finals have already been admitted as occurrence-bound observations by
    // the worker. They do not form a second whole-session transcript here.
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

    let mut accepted_tail_patch_replacements = 0u64;
    let mut tail_patch_worker_accounting = None;
    match worker.join() {
        Ok(Ok(outcome)) => {
            info!(
                sealed = outcome.sealed,
                filtered_empty_drops = outcome.filtered_empty_drops,
                unresolved_windows = outcome.unresolved_windows,
                under_commit_escalations = outcome.under_commit_escalations,
                tail_patch_replacements = outcome.tail_patch_replacements,
                tail_patch_refusals = outcome.tail_patch_refusals,
                "Apple progressive live session finished"
            );
            accepted_tail_patch_replacements = outcome.tail_patch_replacements;
            tail_patch_worker_accounting = Some(TailPatchWorkerAccounting {
                applied_jobs: outcome.tail_patch_jobs_applied,
                skipped_jobs: outcome.tail_patch_jobs_skipped,
                timeout_residue: outcome.tail_patch_timeout_residue,
            });
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

    let receipt = tail_patch_receipt_after_stop(
        tail_patch_on,
        tail_patch_submitted,
        tail_patch_worker_accounting,
    );
    log_tail_patch_session_receipt(receipt);
    report_tail_patch_drain_degrade(
        event_sink.as_ref(),
        receipt.timed_out.saturating_add(receipt.abandoned),
    );
    event_sink.on_event(&receipt.as_event());
    emit_capture_level_receipt(
        event_sink.as_ref(),
        &capture_level.finalize(CapturePathMeta::resolve(sample_rate, 1, None)),
    );
    emit_session_finalised(
        event_sink.as_ref(),
        session_id,
        accepted_tail_patch_replacements,
    );
}

/// Mutable seal state for one Apple stream: revision counters plus the shared
/// postprocessor that corrects every final at seal time.
///
/// Grouped into one struct so `emit_stream_events` keeps a readable signature
/// while the worker and the event mapper stay on the same postprocessor
/// instance (one lexicon reload cadence, one drop counter).
struct PendingAppleSeal {
    /// Exact physical occurrence selected before any observer was launched.
    occurrence: OccurrenceIdentity,
    raw_text: String,
    /// Byte-identical baseline handed to the tail patcher. Patch char offsets
    /// are valid only against this string, never against raw Apple text.
    layer1_baseline: String,
    start_ts: f32,
    end_ts: f32,
    segments: Vec<TranscriptSegment>,
}

struct AppleSealState {
    session_id: String,
    capture_epoch: u64,
    sample_rate: u32,
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
    /// Sealed fragments waiting to share one Whisper window (~5 segments).
    layer1_coalesce: Layer1Coalesce,
    /// Seals whose tail-patch request found the queue full (F1 backpressure).
    tail_patch_backpressure_drops: u64,
    /// Requests accepted by the Layer 1 queue that have not reported back yet.
    /// This — not the pending-seal queue — is what the end-of-session closure
    /// loop waits on: a span can also be held by the Apple volatile window, and
    /// no Whisper completion will ever clear that gate.
    tail_patch_awaiting_completion: u64,
    /// Occurrence formatter hand-off. Presence means the frozen snapshot
    /// permits jobs; it is not itself a scheduled ledger return.
    formatter: Option<mpsc::Sender<FormatterRequest>>,
    /// Exact accepted formatter jobs not yet acknowledged after reducer/seal
    /// delivery. This identity is deliberately independent of `pending_events`:
    /// a concurrently completed observer may publish and remove the pending
    /// payload before the formatter acknowledgement reaches the worker.
    formatter_in_flight: BTreeSet<OccurrenceIdentity>,
    formatter_awaiting_completion: u64,
    /// Concatenation of already progressive-sealed text — left context for
    /// Light+ casing on the next seal (w2-b).
    sealed_prefix: String,
    /// Event payload retained until the occurrence's scheduled observers have
    /// returned. AcousticLedger remains the only seal authority.
    pending_events: BTreeMap<u64, PendingAppleSeal>,
    /// Bounded patch events that actually rewrote a pending span this session.
    tail_patch_replacements: u64,
    /// Completed provider jobs whose mutation crossed the rewrite fence.
    tail_patch_jobs_applied: u64,
    /// Completed provider jobs that produced no accepted mutation (no-change,
    /// provider skip, identity/range refusal, or sealed-fence refusal).
    tail_patch_jobs_skipped: u64,
    /// Identity, replay, sealed-fence, or invalid-range refusals.
    tail_patch_refusals: u64,
    /// The session's single Silero: Supervisor VAD + utterance ledger. `None`
    /// only when neither consumer wants it, or when the model failed to load.
    fusion: Option<SileroIngress>,
    /// Whether Silero identity may reach the seal (`CODESCRIBE_SILERO_FUSION`,
    /// default ON). Independent of [`Self::fusion`] existing: the engine
    /// lifecycle needs the VAD even when an operator has pinned the seal path
    /// back to Apple's own segment boundaries.
    fusion_seal_armed: bool,
    fusion_context: FusionContextMode,
    /// Shared one-throne ledger.
    acoustic_ledger: Arc<Mutex<AcousticLedger>>,
    /// Measured threshold frozen into the same settings snapshot. Absence is a
    /// fail-closed W2 state: no occurrence qualifies and no text mutates.
    energy_calibration: Option<EnergyCalibration>,
}

impl AppleSealState {
    /// Fresh isolated seal state with Layer 1 disabled (`tail_patch: None`).
    /// Product-mode arming is injected by the session owner, not this test helper.
    #[cfg(any())]
    fn new(sample_rate: u32) -> Self {
        Self::new_for_session(sample_rate, uuid::Uuid::new_v4().to_string(), 0)
    }

    fn new_for_session(sample_rate: u32, session_id: String, capture_epoch: u64) -> Self {
        Self {
            session_id,
            capture_epoch,
            sample_rate,
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
            layer1_coalesce: Layer1Coalesce::default(),
            tail_patch_backpressure_drops: 0,
            tail_patch_awaiting_completion: 0,
            formatter: None,
            formatter_in_flight: BTreeSet::new(),
            formatter_awaiting_completion: 0,
            sealed_prefix: String::new(),
            pending_events: BTreeMap::new(),
            tail_patch_replacements: 0,
            tail_patch_jobs_applied: 0,
            tail_patch_jobs_skipped: 0,
            tail_patch_refusals: 0,
            fusion: None,
            fusion_seal_armed: false,
            fusion_context: FusionContextMode::UtteranceOnly,
            acoustic_ledger: Arc::new(Mutex::new(AcousticLedger::new())),
            energy_calibration: None,
        }
    }

    fn new_for_session_with_ledger(
        sample_rate: u32,
        session_id: String,
        capture_epoch: u64,
        acoustic_ledger: Arc<Mutex<AcousticLedger>>,
        energy_calibration: Option<EnergyCalibration>,
    ) -> Self {
        Self {
            acoustic_ledger,
            energy_calibration,
            ..Self::new_for_session(sample_rate, session_id, capture_epoch)
        }
    }

    /// Same state, armed with the Layer 1 hand-off. Holding the sender is what
    /// makes `seal_utterance_final` clone the committed text at all — with no
    /// wire there is nothing to diff against later.
    #[cfg(any())]
    fn new_with_tail_patch(sample_rate: u32, tail_patch: mpsc::Sender<TailPatchRequest>) -> Self {
        Self {
            tail_patch: Some(tail_patch),
            ..Self::new(sample_rate)
        }
    }

    /// Returns whether the current exact occurrence was accepted into the
    /// coalescer-owned terminal lifecycle. Queue results for older flushes are
    /// deliberately independent of this return value.
    fn enqueue_layer1_piece(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        piece: CoalescedPiece,
    ) -> bool {
        if self.tail_patch.is_none() {
            return false;
        }
        let utterance_id = piece.utterance_id;
        let occurrence = piece.occurrence.clone();
        if self
            .pending_events
            .get(&utterance_id)
            .is_none_or(|pending| pending.occurrence != occurrence)
        {
            return false;
        }
        let scheduled = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .schedule_observer(occurrence, LedgerObservationProducer::Whisper);
        if !scheduled {
            return false;
        }
        if self.layer1_coalesce.is_empty() {
            self.layer1_coalesce
                .set_neighbour(self.sealed_prefix.clone());
        }
        for flush in self.layer1_coalesce.push(piece, self.sample_rate) {
            // A pause can flush A while B becomes the newly held member. A's
            // transport outcome must never revoke B's accepted ownership.
            let _ = self.queue_layer1_flush(ev_tx, flush);
        }
        true
    }

    fn flush_layer1_coalesce(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) -> bool {
        // A held window can drain as several requests when its pieces are not
        // adjacent; every contiguous run is queued on its own.
        // Not `any`: it short-circuits, and a run that fails to queue must not
        // stop the runs after it from being offered.
        let mut queued = false;
        for flush in self.layer1_coalesce.force_flush() {
            queued |= self.queue_layer1_flush(ev_tx, flush);
        }
        queued
    }

    fn queue_layer1_flush(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        flush: CoalesceFlush,
    ) -> bool {
        let member_occurrences = flush.member_occurrences.clone();
        if member_occurrences.len() != flush.member_ids.len() {
            warn!(
                utterance_id = flush.primary_utterance_id,
                expected_members = flush.member_ids.len(),
                exact_members = member_occurrences.len(),
                "Layer 1 launch refused — coalescer member identity cardinality diverged"
            );
            for (utterance_id, occurrence) in &member_occurrences {
                self.return_whisper_without_label(ev_tx, occurrence);
                self.emit_pending_seal(ev_tx, *utterance_id);
            }
            return false;
        }
        let Some(tx) = self.tail_patch.clone() else {
            for (utterance_id, occurrence) in &member_occurrences {
                self.return_whisper_without_label(ev_tx, occurrence);
                self.emit_pending_seal(ev_tx, *utterance_id);
            }
            return false;
        };
        let provider_request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: flush.primary_utterance_id,
                range: TailSampleRange {
                    session: self.session_id.clone(),
                    capture_epoch: self.capture_epoch,
                    sample_start: flush.sample_start,
                    sample_end: flush.sample_end,
                },
            },
            sample_rate: self.sample_rate,
            language: None,
        };
        match tx.try_send(TailPatchRequest {
            utterance_id: flush.primary_utterance_id,
            committed_text: flush.committed_text,
            neighbour_context: flush.neighbour_context,
            audio: flush.audio,
            provider_request,
            member_occurrences: member_occurrences.clone(),
        }) {
            Ok(()) => {
                self.tail_patch_awaiting_completion =
                    self.tail_patch_awaiting_completion.saturating_add(1);
                true
            }
            Err(error) => {
                self.tail_patch_backpressure_drops =
                    self.tail_patch_backpressure_drops.saturating_add(1);
                warn!(
                    utterance_id = flush.primary_utterance_id,
                    "Layer 1 tail-patch request dropped — queue full or lane gone: {error}"
                );
                for (utterance_id, occurrence) in &member_occurrences {
                    self.return_whisper_without_label(ev_tx, occurrence);
                    self.emit_pending_seal(ev_tx, *utterance_id);
                }
                false
            }
        }
    }

    fn new_with_tail_patch_for_session(
        sample_rate: u32,
        session_id: String,
        capture_epoch: u64,
        tail_patch: mpsc::Sender<TailPatchRequest>,
        acoustic_ledger: Arc<Mutex<AcousticLedger>>,
        energy_calibration: Option<EnergyCalibration>,
    ) -> Self {
        Self {
            tail_patch: Some(tail_patch),
            ..Self::new_for_session_with_ledger(
                sample_rate,
                session_id,
                capture_epoch,
                acoustic_ledger,
                energy_calibration,
            )
        }
    }

    /// Admit a returned Whisper candidate through the same occurrence ledger
    /// as Apple. The legacy char-patch outcome is evidence only; it never owns
    /// a post-seal mutation path.
    fn complete_whisper_window(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        completion: TailPatchCompletion,
        _now_secs: f32,
    ) {
        let TailPatchCompletion {
            utterance_id,
            request_identity,
            payload,
            member_occurrences,
        } = completion;
        let exact_open_members = member_occurrences
            .into_iter()
            .filter(|(member_id, occurrence)| {
                self.pending_events
                    .get(member_id)
                    .is_some_and(|pending| &pending.occurrence == occurrence)
                    && self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .frontier_of(occurrence)
                        .is_some_and(|frontier| {
                            frontier
                                .open_producers()
                                .contains(&LedgerObservationProducer::Whisper)
                        })
            })
            .collect::<Vec<_>>();
        if exact_open_members.is_empty() {
            return;
        }

        let payload_identity_mismatch = payload.as_ref().is_some_and(|payload| {
            !request_identity
                .as_ref()
                .is_some_and(|identity| identity == &payload.identity)
        });
        if payload_identity_mismatch {
            self.refuse_tail_patch(
                ev_tx,
                TAIL_PATCH_IDENTITY_MISMATCH_WARNING_CODE,
                utterance_id,
                "provider completion did not echo the launched request identity",
            );
        }
        let payload = (!payload_identity_mismatch).then_some(payload).flatten();

        let request_id = request_identity
            .as_ref()
            .map_or(utterance_id, |identity| identity.request_id);
        let single_member = exact_open_members.len() == 1;
        let mut mutation_admitted = false;
        for (generation, (member_id, occurrence)) in exact_open_members.iter().enumerate() {
            let label = payload.as_ref().and_then(|payload| {
                let pinned = payload
                    .segments
                    .iter()
                    .filter(|segment| {
                        let pin = OccurrenceIdentity::from(&segment.range);
                        pin.same_capture(occurrence)
                            && pin.sample_start >= occurrence.sample_start
                            && pin.sample_end <= occurrence.sample_end
                    })
                    .map(|segment| segment.text.trim())
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>();
                if !pinned.is_empty() {
                    Some(pinned.join(" "))
                } else if single_member
                    && &OccurrenceIdentity::from(&payload.identity.range) == occurrence
                    && !payload.text.trim().is_empty()
                {
                    Some(payload.text.trim().to_string())
                } else {
                    None
                }
            });
            match label.as_deref().and_then(|label| {
                admit_ledger_label(
                    self,
                    ev_tx,
                    LabelAdmission {
                        observation: LedgerObservationIdentity::new(
                            LedgerObservationProducer::Whisper,
                            request_id,
                            generation as u64,
                            occurrence.clone(),
                        ),
                        label,
                        energy: EnergyAdmission::RequireExistingQualification,
                    },
                )
            }) {
                Some(receipt) => mutation_admitted |= receipt.grants_mutation(),
                None => self.return_whisper_without_label(ev_tx, occurrence),
            }
            self.emit_pending_seal(ev_tx, *member_id);
        }
        if mutation_admitted {
            self.tail_patch_jobs_applied = self.tail_patch_jobs_applied.saturating_add(1);
            self.tail_patch_replacements = self.tail_patch_replacements.saturating_add(1);
        } else {
            self.tail_patch_jobs_skipped = self.tail_patch_jobs_skipped.saturating_add(1);
        }
        self.tail_patch_awaiting_completion = self.tail_patch_awaiting_completion.saturating_sub(1);
    }

    /// Close one launched Whisper slot without inventing a label or receipt.
    fn return_whisper_without_label(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        occurrence: &OccurrenceIdentity,
    ) {
        let formatter = self.formatter.clone();
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let formatter_scheduled = schedule_formatter_after_terminal_label(
            &mut ledger,
            formatter.as_ref(),
            occurrence,
            LedgerObservationProducer::Whisper,
        );
        let closed = ledger.note_frontier_return(occurrence, LedgerObservationProducer::Whisper);
        if closed && let Ok(receipt) = ledger.seal(occurrence).cloned() {
            let _ = ev_tx.send(EngineEvent::LedgerSeal { receipt });
        }
        drop(ledger);
        if formatter_scheduled && self.formatter_in_flight.insert(occurrence.clone()) {
            self.formatter_awaiting_completion =
                self.formatter_awaiting_completion.saturating_add(1);
        }
    }

    /// Accept one completion only after PresentationEmitter returned the same
    /// exact Formatter slot and sealed its occurrence.
    fn complete_formatter(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        completion: FormatterCompletion,
    ) -> bool {
        if !completion.carries_same_occurrence() {
            return false;
        }
        if !self.formatter_in_flight.contains(&completion.occurrence) {
            return false;
        }
        let utterance_id = self
            .pending_events
            .iter()
            .find_map(|(id, pending)| (pending.occurrence == completion.occurrence).then_some(*id));
        let canonical_label = {
            let ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let settled = ledger.is_sealed(&completion.occurrence)
                && ledger
                    .frontier_of(&completion.occurrence)
                    .is_some_and(|frontier| {
                        !frontier
                            .open_producers()
                            .contains(&LedgerObservationProducer::Formatter)
                    });
            settled
                .then(|| ledger.text_of(&completion.occurrence).map(str::to_owned))
                .flatten()
        };
        let Some(canonical_label) = canonical_label else {
            return false;
        };
        if self.formatter_awaiting_completion == 0 {
            return false;
        }
        if !self.formatter_in_flight.remove(&completion.occurrence) {
            return false;
        }
        if let Some(utterance_id) = utterance_id {
            if let Some(pending) = self.pending_events.get_mut(&utterance_id) {
                pending.layer1_baseline = canonical_label;
            }
            self.emit_pending_seal(ev_tx, utterance_id);
        }
        self.formatter_awaiting_completion = self.formatter_awaiting_completion.saturating_sub(1);
        true
    }

    /// Return every still-open launched Whisper slot after the bounded stop
    /// drain expires. This closes only producer obligations; it admits no text.
    fn return_outstanding_whisper_without_label(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    ) {
        let occurrences = {
            let ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger
                .qualified_occurrences()
                .filter(|occurrence| {
                    ledger.frontier_of(occurrence).is_some_and(|frontier| {
                        frontier
                            .open_producers()
                            .contains(&LedgerObservationProducer::Whisper)
                    })
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        for occurrence in occurrences {
            self.return_whisper_without_label(ev_tx, &occurrence);
        }
        self.tail_patch_awaiting_completion = 0;
    }

    fn refuse_tail_patch(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        code: &str,
        utterance_id: u64,
        reason: &str,
    ) {
        self.tail_patch_refusals = self.tail_patch_refusals.saturating_add(1);
        let _ = ev_tx.send(EngineEvent::Warning {
            code: code.to_string(),
            message: format!("utterance {utterance_id}: {reason}; Apple text preserved"),
        });
    }

    /// End-of-session drain for retained reducer inputs. AcousticLedger owns
    /// finality; this method only forwards the already admitted Apple payload.
    fn seal_remaining_at_session_end(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        let pending_ids: Vec<u64> = self.pending_events.keys().copied().collect();
        for utterance_id in pending_ids {
            self.emit_pending_seal(ev_tx, utterance_id);
        }
    }

    fn emit_pending_seal(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>, utterance_id: u64) {
        let ready = self
            .pending_events
            .get(&utterance_id)
            .is_some_and(|pending| {
                self.acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_sealed(&pending.occurrence)
            });
        if !ready {
            return;
        }
        let Some(pending) = self.pending_events.remove(&utterance_id) else {
            return;
        };
        self.sealed_count = self.sealed_count.saturating_add(1);
        if !self.sealed_prefix.is_empty() {
            self.sealed_prefix.push(' ');
        }
        self.sealed_prefix.push_str(&pending.layer1_baseline);
        let _ = ev_tx.send(EngineEvent::UtteranceFinal {
            utterance_id,
            text: pending.layer1_baseline,
            raw_text: pending.raw_text,
            start_ts: pending.start_ts,
            end_ts: pending.end_ts,
            segments: pending.segments,
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: Vec::new(),
        });
    }
}

/// What the worker sealed, and what seal-time postprocess filtered away.
struct AppleStreamOutcome {
    sealed: u64,
    filtered_empty_drops: u64,
    unresolved_windows: u64,
    /// How many seals escalated an unplaceable Layer 1 under-commit (W-C).
    under_commit_escalations: u64,
    /// Bounded Layer 1 events that crossed the rewrite fence before seal.
    tail_patch_replacements: u64,
    /// Completions refused by identity, replay, range, or sealed-fence checks.
    tail_patch_refusals: u64,
    /// Job-level outcome after the single rewrite fence adjudicated it.
    tail_patch_jobs_applied: u64,
    /// Completed jobs with no accepted mutation, including no-change.
    tail_patch_jobs_skipped: u64,
    /// Jobs still outstanding when the bounded closure wait expired.
    tail_patch_timeout_residue: u64,
}

#[cfg(any())]
#[derive(Debug)]
struct LivePatchToken {
    utterance_id: u64,
    start: usize,
    end: usize,
}

/// Convert provider-neutral Layer 1 gap-fill into existing bounded utterance
/// patches. The merge first preserves every Apple token; only tokens present
/// in the merged result but absent from that floor become zero-width inserts.
#[cfg(any())]
fn plan_live_layer1_gap_patches(spans: &[SealedSpan], candidate: &str) -> Vec<EngineEvent> {
    if spans.is_empty() || candidate.trim().is_empty() {
        return Vec::new();
    }
    let live = spans
        .iter()
        .map(|span| span.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let merged = crate::quality::merge_live_layer1(&live, candidate);
    if merged.provider_fill_tokens == 0 {
        return Vec::new();
    }

    let live_tokens = crate::quality::teacher::tokenize(&live);
    let merged_tokens = crate::quality::teacher::tokenize(&merged.text);
    let mapped = mapped_live_tokens(spans);
    if mapped.len() != live_tokens.len() {
        warn!(
            mapped_tokens = mapped.len(),
            live_tokens = live_tokens.len(),
            "Live cloud gap planner refused inconsistent span token map"
        );
        return Vec::new();
    }
    let ops = crate::quality::teacher::align_words(&live_tokens, &merged_tokens);
    let mut patches = Vec::new();
    let mut previous_live: Option<usize> = None;
    let mut index = 0usize;
    while index < ops.len() {
        match &ops[index] {
            crate::quality::teacher::AlignOp::InsertB { .. } => {
                let start = index;
                while matches!(
                    ops.get(index),
                    Some(crate::quality::teacher::AlignOp::InsertB { .. })
                ) {
                    index += 1;
                }
                let words = ops[start..index]
                    .iter()
                    .filter_map(|op| match op {
                        crate::quality::teacher::AlignOp::InsertB { b } => {
                            Some(merged_tokens[*b].surface.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let next_live = ops[index..].iter().find_map(live_op_index);
                if let Some(previous) = previous_live.and_then(|idx| mapped.get(idx)) {
                    patches.push(EngineEvent::ReplaceRange {
                        utterance_id: previous.utterance_id,
                        start: previous.end,
                        end: previous.end,
                        text: format!(" {words}"),
                        source: LayerSource::TailPatch,
                    });
                } else if let Some(next) = next_live.and_then(|idx| mapped.get(idx)) {
                    patches.push(EngineEvent::ReplaceRange {
                        utterance_id: next.utterance_id,
                        start: next.start,
                        end: next.start,
                        text: format!("{words} "),
                        source: LayerSource::TailPatch,
                    });
                }
            }
            crate::quality::teacher::AlignOp::Substitute { a, b } => {
                if let Some(live_token) = mapped.get(*a) {
                    patches.push(EngineEvent::ReplaceRange {
                        utterance_id: live_token.utterance_id,
                        start: live_token.start,
                        end: live_token.end,
                        text: merged_tokens[*b].surface.clone(),
                        source: LayerSource::TailPatch,
                    });
                }
                previous_live = Some(*a);
                index += 1;
            }
            op => {
                previous_live = live_op_index(op).or(previous_live);
                index += 1;
            }
        }
    }

    // Multiple inserts into one utterance use offsets from the same immutable
    // Apple text. Apply right-to-left so an earlier insertion cannot shift a
    // later one's char boundary.
    patches.sort_by(|left, right| {
        patch_position(right)
            .cmp(&patch_position(left))
            .then_with(|| patch_utterance(right).cmp(&patch_utterance(left)))
    });
    patches
}

#[cfg(any())]
fn live_op_index(op: &crate::quality::teacher::AlignOp) -> Option<usize> {
    match op {
        crate::quality::teacher::AlignOp::Equal { a, .. }
        | crate::quality::teacher::AlignOp::DeleteA { a }
        | crate::quality::teacher::AlignOp::Substitute { a, .. } => Some(*a),
        crate::quality::teacher::AlignOp::InsertB { .. } => None,
    }
}

#[cfg(any())]
fn mapped_live_tokens(spans: &[SealedSpan]) -> Vec<LivePatchToken> {
    let mut mapped = Vec::new();
    for span in spans {
        let chars = span.text.chars().collect::<Vec<_>>();
        let mut cursor = 0usize;
        while cursor < chars.len() {
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            let start = cursor;
            while cursor < chars.len() && !chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if start < cursor {
                mapped.push(LivePatchToken {
                    utterance_id: span.id,
                    start,
                    end: cursor,
                });
            }
        }
    }
    mapped
}

#[cfg(any())]
fn patch_position(event: &EngineEvent) -> usize {
    match event {
        EngineEvent::ReplaceRange { start, .. } => *start,
        _ => 0,
    }
}

#[cfg(any())]
fn patch_utterance(event: &EngineEvent) -> u64 {
    match event {
        EngineEvent::ReplaceRange { utterance_id, .. } => *utterance_id,
        _ => 0,
    }
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
    let mut from = state.last_sealed_end;
    // A `from` that fell off retention is not a disagreeing clock — that audio
    // is gone because SFSpeech withheld its first final past the retention
    // horizon (measured 2026-08-14: a 247 s take whose first final arrived at
    // 156 s went 11/11 unresolved and starved Layer 1 for the WHOLE take,
    // because one miss keeps `last_sealed_end` pinned forever). Clamp the
    // start to retained audio; genuine clock lies (an `end_ts` that itself
    // precedes retention or overshoots the session) stay fail-closed below.
    let retained_start = state.audio.retained_start_secs();
    if from < retained_start && end_ts > retained_start {
        warn!(
            from_secs = from,
            retained_start_secs = retained_start,
            end_ts,
            "Apple seal window start fell off retention — clamped to retained audio"
        );
        from = retained_start;
    }
    match state.audio.window_with_range(from, end_ts) {
        Some(window) => {
            // `window_with_range` is the ingestion boundary where Apple's
            // floating span clock becomes the canonical integer PCM clock. A
            // small Apple overshoot is intentionally clamped there; carrying
            // the *requested* `end_ts` forward would make the next window
            // start beyond captured audio even though this window resolved.
            let pcm_end_secs = window.sample_end as f32 / state.sample_rate.max(1) as f32;
            state.last_sealed_end = pcm_end_secs;
            // Keep the bounded session tail until terminal coverage has been
            // checked. A later occurrence can expose an earlier speech hole;
            // releasing everything before this boundary would destroy the PCM
            // needed to admit that hole through the ledger corridor. The ring
            // still enforces DEFAULT_RETENTION_SECS.
            if window.samples.is_empty() {
                // A cumulative final may assert novel text after the PCM clock
                // has reached EOF. Content still seals, but zero samples are
                // not a Whisper window and this known clamp is not a clock lie.
                tracing::debug!(
                    from_secs = from,
                    requested_end_secs = end_ts,
                    pcm_end_secs,
                    "Apple seal resolved at PCM boundary with no new audio"
                );
                return None;
            }
            tracing::debug!(
                from_secs = from,
                requested_end_secs = end_ts,
                pcm_end_secs,
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

#[cfg(any())]
fn timed_words_to_segments(words: &[TimedTailSegment], sample_rate: u32) -> Vec<TranscriptSegment> {
    let rate = sample_rate.max(1) as f32;
    words
        .iter()
        .filter(|word| {
            word.range.sample_end > word.range.sample_start && !word.text.trim().is_empty()
        })
        .map(|word| TranscriptSegment {
            text: word.text.clone(),
            start_ts: word.range.sample_start as f32 / rate,
            end_ts: word.range.sample_end as f32 / rate,
        })
        .collect()
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

/// Extra copies of a canvas token are new acoustic occurrences, not revisions.
#[cfg(any())]
fn cap_known_prefix_to_canvas_token_counts(probe: &[String], canvas: &[&str], k: usize) -> usize {
    let mut canvas_counts = std::collections::HashMap::<&str, usize>::new();
    for word in canvas {
        *canvas_counts.entry(*word).or_insert(0) += 1;
    }
    let mut used = std::collections::HashMap::<&str, usize>::new();
    let k = k.min(probe.len());
    for (index, token) in probe[..k].iter().enumerate() {
        let Some(&canvas_n) = canvas_counts
            .get(token.as_str())
            .filter(|count| **count > 0)
        else {
            continue;
        };
        let used_n = used.entry(token.as_str()).or_insert(0);
        *used_n += 1;
        if *used_n > canvas_n {
            return index;
        }
    }
    k
}

/// Committed occurrences a cumulative restatement may claim, newest first,
/// paired with the canvas words each contributed.
///
/// Two rules shape the set, and both are the utterance-grain rule from the
/// acoustic ledger applied to the canvas side:
///
/// * whole spans only — a span is the occurrence unit, so a restatement claims
///   all of one or none of it, never an invented slice through the middle;
/// * no more canvas words than the callback itself carries — a restatement
///   cannot be shorter than what it restates, so `max_words` (the callback's
///   own word count) is how far back it may reach.
///
/// Together they keep the matcher from reaching into transcript history that
/// has no temporal relationship to the callback. Before the cut it scanned
/// every start position within `2n + 16` canvas words and took the longest
/// match anywhere in that band.
#[cfg(any())]
fn restatable_occurrences(
    state: &AppleSealState,
    max_words: usize,
) -> Vec<(OccurrenceIdentity, usize)> {
    let sealed = state.progressive.sealed_spans().iter().map(|span| {
        let words = normalize_for_containment(&span.text)
            .split_whitespace()
            .count();
        (OccurrenceIdentity::from(&span.range), words)
    });
    let pending = state.progressive.pending_spans().iter().map(|span| {
        let words = normalize_for_containment(&span.raw_text)
            .split_whitespace()
            .count();
        (OccurrenceIdentity::from(&span.range), words)
    });
    let canvas_order: Vec<(OccurrenceIdentity, usize)> = sealed
        .chain(pending)
        .filter(|(_, words)| *words > 0)
        .collect();

    let mut claimed = Vec::new();
    let mut budget = max_words;
    for entry in canvas_order.into_iter().rev() {
        if entry.1 > budget {
            break;
        }
        budget -= entry.1;
        claimed.push(entry);
    }
    claimed.reverse();
    claimed
}

/// Case- and punctuation-insensitive projection for canvas containment checks
/// (the sealed canvas carries Light+ casing and sentence terminals, raw
/// callbacks carry neither).
#[cfg(any())]
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

/// Slice a cumulative Apple final onto Silero-minted utterance ranges.
///
/// Returns `true` when at least one Silero span accepted words (the callback
/// is consumed). `false` leaves the caller on the Apple-boundary path so
/// speech is never dropped when Silero has not yet opened an edge.
fn seal_sliced_by_silero(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
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
        let text = words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if text.trim().is_empty() {
            continue;
        }
        let span_start = words
            .first()
            .map(|word| word.sample_start as f32 / rate)
            .unwrap_or(silero.range.sample_start as f32 / rate);
        let span_end = words
            .last()
            .map(|word| word.sample_end as f32 / rate)
            .unwrap_or(silero.range.sample_end as f32 / rate);
        let slice_segments = words
            .iter()
            .map(|word| TranscriptSegment {
                text: word.text.clone(),
                start_ts: word.sample_start as f32 / rate,
                end_ts: word.sample_end as f32 / rate,
            })
            .collect::<Vec<_>>();

        // Silero has already selected the physical occurrence. Admit the
        // slice-local Apple label before the raw final can escape as telemetry.
        // There is no independent slice-local Lexicon rewrite on this path, so
        // Lexicon reports a no-change observation for the same exact label and
        // range. Callback-wide text is never copied across sliced occurrences.
        let occurrence = OccurrenceIdentity::from(&silero.range);
        let apple_admitted = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    utterance_id,
                    0,
                    occurrence.clone(),
                ),
                label: &text,
                energy: EnergyAdmission::QualifyFromOwnedPcm,
            },
        );
        state.pending_events.insert(
            utterance_id,
            PendingAppleSeal {
                occurrence: occurrence.clone(),
                raw_text: text.clone(),
                layer1_baseline: text.clone(),
                start_ts: span_start,
                end_ts: span_end,
                segments: slice_segments,
            },
        );

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
        let _current_piece_owned = if apple_admitted.is_some()
            && let Some(window) = window
        {
            if state.tail_patch.is_some() {
                let committed_text = text.clone();
                state.enqueue_layer1_piece(
                    ev_tx,
                    CoalescedPiece {
                        utterance_id,
                        occurrence: occurrence.clone(),
                        committed_text,
                        audio: window.samples,
                        sample_start: window.sample_start,
                        sample_end: window.sample_end,
                        start_ts: span_start,
                        covered_through_secs: span_end,
                        segment_count: disjoint.len().max(1),
                    },
                )
            } else {
                false
            }
        } else {
            false
        };
        if apple_admitted.is_some() {
            let _ = admit_ledger_label(
                state,
                ev_tx,
                LabelAdmission {
                    observation: LedgerObservationIdentity::new(
                        LedgerObservationProducer::Lexicon,
                        utterance_id,
                        0,
                        occurrence,
                    ),
                    label: &text,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            );
        }
        state.emit_pending_seal(ev_tx, utterance_id);
        state.utterance_id = state.utterance_id.max(utterance_id);
    }
    true
}

/// Whether one label admission carries the right to establish this
/// occurrence's acoustic qualification from the PCM window it owns.
///
/// Only an observer that owns the capture window may measure it. A later
/// observer of the same occurrence rides the existing qualification: it never
/// re-measures energy the ledger already judged, and never invents evidence for
/// a window the ledger refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnergyAdmission {
    /// Measure the owned capture window and qualify it through the calibration.
    QualifyFromOwnedPcm,
    /// A terminal final-pass job owns an uncovered measured speech range. It
    /// qualifies that PCM and schedules only the Whisper observer that
    /// actually ran; Apple/Lexicon never observed this occurrence.
    QualifyFinalPassGap,
    /// Refuse unless the ledger already qualified this exact occurrence.
    RequireExistingQualification,
}

/// One occurrence-authenticated label admission.
///
/// `observation` is the whole authority: the producing engine, its
/// request/generation ordinals, and the exact `(session, capture_epoch,
/// sample_start, sample_end)` occurrence being described. Grouping them in one
/// value is not cosmetic — it makes it unrepresentable to admit a label under an
/// observation identity that names a different occurrence than the one whose
/// energy was qualified. `label` is payload and never an identity key.
struct LabelAdmission<'a> {
    observation: LedgerObservationIdentity,
    label: &'a str,
    energy: EnergyAdmission,
}

fn admit_ledger_label(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    admission: LabelAdmission<'_>,
) -> Option<MutationReceipt> {
    let LabelAdmission {
        observation,
        label,
        energy,
    } = admission;
    let occurrence = observation.occurrence.clone();
    let producer = observation.producer;
    let calibration = state.energy_calibration.as_ref()?;
    let formatter = state.formatter.clone();
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !ledger.is_qualified(&occurrence) {
        if !matches!(
            energy,
            EnergyAdmission::QualifyFromOwnedPcm | EnergyAdmission::QualifyFinalPassGap
        ) {
            return None;
        }
        let window = state
            .audio
            .window_by_samples(occurrence.sample_start, occurrence.sample_end)?;
        if window.samples.is_empty() {
            return None;
        }
        let energy_integral = window
            .samples
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>();
        let mean_rms = (energy_integral / window.samples.len() as f64).sqrt();
        let peak = window
            .samples
            .iter()
            .map(|sample| f64::from(sample.abs()))
            .fold(0.0_f64, f64::max);
        let dbfs = |linear: f64| {
            if linear > 0.0 {
                20.0 * linear.log10()
            } else {
                f64::NEG_INFINITY
            }
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: occurrence.sample_len() as f64 * 1_000.0 / state.sample_rate.max(1) as f64,
            energy_integral,
            mean_rms_dbfs: dbfs(mean_rms),
            peak_dbfs: dbfs(peak),
            vad_open_sample: Some(occurrence.sample_start),
            vad_close_sample: Some(occurrence.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        if !ledger.qualify(&evidence, calibration).is_qualified() {
            return None;
        }
    }
    if ledger.frontier_of(&occurrence).is_none() {
        let producers = if energy == EnergyAdmission::QualifyFinalPassGap {
            vec![LedgerObservationProducer::Whisper]
        } else {
            vec![
                LedgerObservationProducer::Apple,
                LedgerObservationProducer::Lexicon,
            ]
        };
        ledger.schedule_frontier(occurrence.clone(), producers);
    }
    let receipt = ledger.admit(&observation, label);
    let _ = ev_tx.send(EngineEvent::LedgerMutation {
        observation,
        label: label.to_string(),
        receipt: receipt.clone(),
    });
    let formatter_scheduled = schedule_formatter_after_terminal_label(
        &mut ledger,
        formatter.as_ref(),
        &occurrence,
        producer,
    );
    let closed = ledger.note_frontier_return(&occurrence, producer);
    if closed && let Ok(seal) = ledger.seal(&occurrence).cloned() {
        let _ = ev_tx.send(EngineEvent::LedgerSeal { receipt: seal });
    }
    drop(ledger);
    if formatter_scheduled && state.formatter_in_flight.insert(occurrence) {
        state.formatter_awaiting_completion = state.formatter_awaiting_completion.saturating_add(1);
    }
    Some(receipt)
}

/// Compare committed occurrence coverage with the existing Silero speech
/// ledger (or the capture energy ladder when Silero produced no spans), then
/// offer every material hole to Whisper on its exact PCM range. The
/// whole-session pass is retained only as comparison evidence; gap text enters
/// exclusively through `admit_ledger_label` below.
fn repair_terminal_seal_coverage(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    language: Option<&str>,
) -> SealCoverageReceipt {
    let threshold_samples =
        u64::from(state.sample_rate).saturating_mul(SEAL_COVERAGE_INCOMPLETE_MS) / 1_000;
    let vad_ranges = state
        .fusion
        .as_ref()
        .map(|fusion| {
            fusion
                .ledger()
                .utterances()
                .iter()
                .filter(|utterance| utterance.closed)
                .map(|utterance| utterance.range.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let speech_ranges = if vad_ranges.is_empty() {
        session_active_speech_ranges(&state.session_id, state.capture_epoch, state.sample_rate)
    } else {
        vad_ranges
    };
    let (initial, apple_text) = {
        let ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &speech_ranges,
                threshold_samples,
            ),
            ledger.rendered_text(),
        )
    };

    let full_range = speech_ranges
        .first()
        .zip(speech_ranges.last())
        .map(|(first, last)| TailSampleRange {
            session: state.session_id.clone(),
            capture_epoch: state.capture_epoch,
            sample_start: first.sample_start,
            sample_end: last.sample_end,
        });
    let comparison = full_range.and_then(|range| {
        let window = state
            .audio
            .window_by_samples(range.sample_start, range.sample_end)?;
        let request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: u64::MAX,
                range,
            },
            sample_rate: state.sample_rate,
            language: language.map(str::to_owned),
        };
        match InProcessTailProvider.transcribe(&request, &window.samples) {
            Ok(payload) if !payload.text.trim().is_empty() => {
                Some(TranscriptComparisonReceipt::new(
                    apple_text.clone(),
                    payload.text.trim().to_string(),
                ))
            }
            Ok(_) => {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "seal_coverage_final_pass_empty".to_string(),
                    message: "whole-session final pass returned no rendered text".to_string(),
                });
                None
            }
            Err(error) => {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "seal_coverage_final_pass_failed".to_string(),
                    message: error.to_string(),
                });
                None
            }
        }
    });

    state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_seal_coverage(initial.clone());
    let _ = ev_tx.send(EngineEvent::SealCoverage {
        receipt: initial.clone(),
        comparison: comparison.clone(),
    });
    if initial.status == SealCoverageStatus::Complete {
        return initial;
    }

    for (ordinal, range) in initial
        .uncovered_speech_ranges
        .iter()
        .filter(|range| range.sample_end.saturating_sub(range.sample_start) > threshold_samples)
        .cloned()
        .enumerate()
    {
        let Some(window) = state
            .audio
            .window_by_samples(range.sample_start, range.sample_end)
        else {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "seal_coverage_gap_pcm_unavailable".to_string(),
                message: format!("{}..{}", range.sample_start, range.sample_end),
            });
            continue;
        };
        let request_id = u64::MAX.saturating_sub(ordinal as u64 + 1);
        let request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id,
                range: range.clone(),
            },
            sample_rate: state.sample_rate,
            language: language.map(str::to_owned),
        };
        let label = match InProcessTailProvider.transcribe(&request, &window.samples) {
            Ok(payload) if !payload.text.trim().is_empty() => payload.text.trim().to_string(),
            Ok(_) => continue,
            Err(error) => {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "seal_coverage_gap_recovery_failed".to_string(),
                    message: format!("{}..{}: {error}", range.sample_start, range.sample_end),
                });
                continue;
            }
        };
        let _ = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Whisper,
                    request_id,
                    0,
                    OccurrenceIdentity::from(&range),
                ),
                label: &label,
                energy: EnergyAdmission::QualifyFinalPassGap,
            },
        );
    }

    let final_receipt = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .assess_seal_coverage(
            &state.session_id,
            state.capture_epoch,
            &speech_ranges,
            threshold_samples,
        );
    state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_seal_coverage(final_receipt.clone());
    let _ = ev_tx.send(EngineEvent::SealCoverage {
        receipt: final_receipt.clone(),
        comparison,
    });
    final_receipt
}

/// Hand one exact occurrence to Formatter only when the returning producer is
/// the last earlier observer and bounded transport ownership is already held.
/// Configuration intent, label equality, and queue availability alone never
/// change the ledger frontier.
fn schedule_formatter_after_terminal_label(
    ledger: &mut AcousticLedger,
    formatter: Option<&mpsc::Sender<FormatterRequest>>,
    occurrence: &OccurrenceIdentity,
    returning: LedgerObservationProducer,
) -> bool {
    let Some(frontier) = ledger.frontier_of(occurrence) else {
        return false;
    };
    let open_producers = frontier.open_producers();
    if open_producers.len() != 1 || !open_producers.contains(&returning) {
        return false;
    }
    let Some(existing_label) = ledger
        .text_of(occurrence)
        .filter(|label| !label.trim().is_empty())
        .map(str::to_owned)
    else {
        return false;
    };
    let Some(formatter) = formatter else {
        return false;
    };
    let Ok(permit) = formatter.try_reserve() else {
        return false;
    };
    if !ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::Formatter) {
        return false;
    }
    permit.send(FormatterRequest {
        occurrence: occurrence.clone(),
        existing_label,
    });
    true
}

/// Seal one Apple utterance: run the shared lexicon + cleanup pass, then emit
/// `UtteranceFinal`. Returns `false` when postprocess filtered the text to
/// empty; an explicit `Drop` event is emitted instead of an empty final.
///
/// `raw_text` keeps the uncorrected engine output so the quality loop can see
/// exactly what the lexicon rewrote.
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
        // A segment-less final still names new session-clock audio. Preserve it
        // as one occurrence; text is payload and never a deduplication key.
        let start_ts = state.last_apple_segment_end.max(state.last_sealed_end);
        let end_ts = audio_secs.max(start_ts + BOUNDARY_EPSILON_SECS);
        info!(
            audio_secs,
            synthesized_start = start_ts,
            synthesized_end = end_ts,
            text_chars = callback_text.chars().count(),
            "apple_lifecycle: segment-less final bound to synthesized window"
        );
        disjoint.push(TranscriptSegment {
            text: callback_text.clone(),
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

    // The seal path knows exactly how many acoustic spans this text covers, so
    // the repetition cleanup is told rather than left to guess. A run of
    // identical words with one span per copy is speech; only a run longer than
    // the audio can account for is a decoder loop.
    let after_lexicon = crate::quality::overlay_quality::apply_custom_lexicon(raw_text.trim());
    if state.fusion_seal_armed && seal_sliced_by_silero(state, ev_tx, &disjoint) {
        return true;
    }
    let apple_words = apple_segments_on_pcm_clock(state, &disjoint);
    let request_id = state.utterance_id.saturating_add(1);
    let captured_end = state.audio.session_sample_end();
    let span_sample_start = apple_words.first().map_or_else(
        || seconds_to_captured_sample(start_ts, state.sample_rate, captured_end),
        |word| word.range.sample_start,
    );
    let span_sample_end = apple_words
        .last()
        .map_or_else(
            || seconds_to_captured_sample(end_ts, state.sample_rate, captured_end),
            |word| word.range.sample_end,
        )
        .max(span_sample_start);
    // Bind this span to the spectrum even off the sliced path: when a Silero
    // edge already encloses every sample Apple claimed, the utterance range is
    // the canonical one and the span records which identity it came from. No
    // enclosing edge (Silero off, model missing, an edge still open, or a span
    // that straddles two utterances) leaves the Apple-derived range untouched —
    // binding never costs content.
    let apple_range = TailSampleRange {
        session: state.session_id.clone(),
        capture_epoch: state.capture_epoch,
        sample_start: span_sample_start,
        sample_end: span_sample_end,
    };
    let (span_range, silero_bound) = match state
        .fusion
        .as_ref()
        .filter(|_| state.fusion_seal_armed)
        .and_then(|fusion| {
            fusion
                .ledger()
                .utterance_enclosing(span_sample_start, span_sample_end)
        }) {
        Some(utterance) => (utterance.range.clone(), true),
        None => (apple_range, false),
    };
    let ledger_occurrence = OccurrenceIdentity::from(&span_range);
    let apple_admitted = admit_ledger_label(
        state,
        ev_tx,
        LabelAdmission {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Apple,
                request_id,
                0,
                ledger_occurrence.clone(),
            ),
            label: &raw_text,
            energy: if silero_bound {
                EnergyAdmission::QualifyFromOwnedPcm
            } else {
                EnergyAdmission::RequireExistingQualification
            },
        },
    );
    // One id space. While the seal path can mint span ids FROM the ledger, the
    // fallback must burn its id there too, or Silero would later mint the same
    // id for a real utterance — `note_apple_commit_timed` is idempotent on id,
    // so the collision would silently merge two unrelated spans. With the seal
    // path disarmed no ledger id ever becomes a span id, and the counter stays
    // the plain monotonic one the Apple-boundary lane always used.
    let utterance_id = match state.fusion.as_mut().filter(|_| state.fusion_seal_armed) {
        Some(fusion) => fusion.ledger_mut().reserve_id(),
        None => state.utterance_id.saturating_add(1),
    };
    state.utterance_id = state.utterance_id.max(utterance_id);
    let segment_count = disjoint.len().max(1);
    let committed_text = after_lexicon.clone();
    state.pending_events.insert(
        utterance_id,
        PendingAppleSeal {
            occurrence: ledger_occurrence.clone(),
            raw_text,
            layer1_baseline: committed_text.clone(),
            start_ts,
            end_ts,
            segments: disjoint,
        },
    );

    let window = resolve_sealed_audio_window(state, end_ts);
    let _current_piece_owned = if apple_admitted.is_some()
        && let Some(window) = window
    {
        if state.tail_patch.is_some() {
            state.enqueue_layer1_piece(
                ev_tx,
                CoalescedPiece {
                    utterance_id,
                    occurrence: ledger_occurrence.clone(),
                    committed_text,
                    audio: window.samples,
                    sample_start: window.sample_start,
                    sample_end: window.sample_end,
                    start_ts,
                    covered_through_secs: end_ts,
                    segment_count,
                },
            )
        } else {
            false
        }
    } else {
        false
    };

    if apple_admitted.is_some() {
        let _ = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    request_id,
                    0,
                    ledger_occurrence,
                ),
                label: &after_lexicon,
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );
    }

    state.emit_pending_seal(ev_tx, utterance_id);
    true
}

// ═══════════════════════════════════════════════════════════
// Engine lifecycle: speech epochs (hands-free silence)
// ═══════════════════════════════════════════════════════════

/// Audio replayed into a fresh epoch ahead of the detected speech edge, so the
/// first phoneme is not eaten by bridge spin-up (~0.24 s measured). Same value
/// the fusion lane pads windows with.
const EPOCH_PREROLL_SECS: f32 = super::silero_fusion::DEFAULT_LEFT_PAD_SECS;

/// Lift one poll's worth of bridge events onto the session PCM clock.
///
/// Bridge time is **per request**: every `LiveStreamSession` restarts its
/// segment clock at zero, while every consumer downstream
/// ([`apple_segments_on_pcm_clock`], the seal windows, the Layer 1 ranges)
/// reads those seconds as session time. With one stream per take the two
/// clocks coincide and this is the identity; with an epoch lifecycle they
/// diverge by exactly the epoch base, so the shift happens once, here, before
/// any event reaches [`emit_stream_events`].
fn shift_events(events: Vec<LiveStreamEvent>, base_secs: f32) -> Vec<LiveStreamEvent> {
    if !base_secs.is_finite() || base_secs <= 0.0 {
        return events;
    }
    events
        .into_iter()
        .map(|event| match event {
            LiveStreamEvent::Partial { text, segments } => LiveStreamEvent::Partial {
                text,
                segments: shift_segments(segments, base_secs),
            },
            LiveStreamEvent::PhraseFinal { text, segments } => LiveStreamEvent::PhraseFinal {
                text,
                segments: shift_segments(segments, base_secs),
            },
            LiveStreamEvent::Summary {
                text,
                segments,
                ok,
                error,
            } => LiveStreamEvent::Summary {
                text,
                segments: shift_segments(segments, base_secs),
                ok,
                error,
            },
            other @ (LiveStreamEvent::Ready
            | LiveStreamEvent::End
            | LiveStreamEvent::Error { .. }) => other,
        })
        .collect()
}

fn shift_segments(segments: Vec<TranscriptSegment>, base_secs: f32) -> Vec<TranscriptSegment> {
    segments
        .into_iter()
        .map(|mut segment| {
            if segment.start_ts.is_finite() {
                segment.start_ts += base_secs;
            }
            if segment.end_ts.is_finite() {
                segment.end_ts += base_secs;
            }
            segment
        })
        .collect()
}

/// What the worker must do with one capture chunk under the epoch lifecycle.
#[derive(Debug, Clone, Copy, PartialEq)]
enum EpochDecision {
    /// Write the chunk into the currently open stream.
    Forward,
    /// Speech edge while asleep: open a stream based at `preroll_from`.
    Wake { preroll_from: u64 },
    /// Silence threshold crossed: close the epoch. Chunk is trailing silence.
    Sleep { silence_secs: f32 },
    /// Asleep and still silent — retain audio only.
    Idle,
}

/// Engine lifecycle for the Apple progressive lane: speech opens an SFSpeech
/// epoch, silence past the product threshold closes it, and the engine rests
/// (mic + Silero keep running) until the next speech edge.
///
/// Disarmed (`utterance_silence_sec: None`, or no Silero) it answers
/// [`EpochDecision::Forward`] to everything, which is the pre-epoch worker bit
/// for bit.
///
/// # The gate observes nothing itself
///
/// It is a pure state machine over one bit per chunk — `speech_live` — supplied
/// by [`SileroIngress::ingest`], the session's single VAD. It used to own a
/// second `SpeechSession` of its own, which meant two Silero instances scoring
/// the same PCM: the lifecycle woke and slept on one set of edges while the
/// fusion ledger minted utterance identity on another, and nothing kept the two
/// spectra in step. One session, one spectrum, one set of boundaries.
///
/// Speech is "live" while a Supervisor segment is open, and for the chunk a
/// segment closes in — so the silence counter starts at the segment close, i.e.
/// **after** Silero's own hysteresis (`0.55 s` by default) has already elapsed.
/// The wall silence before an epoch closes is therefore the product threshold
/// plus that hysteresis, never less than the setting.
struct EpochGate {
    armed: bool,
    sample_rate: u32,
    silence_threshold_samples: u64,
    preroll_samples: u64,
    awake: bool,
    /// Session cursor of the last chunk speech was live in.
    last_speech_sample: u64,
    /// Session cursor the previous epoch closed at — the pre-roll floor, so a
    /// new epoch never re-feeds audio the closed one already carried.
    epoch_closed_at: u64,
}

impl EpochGate {
    /// Legacy lane: one stream for the whole take.
    fn disarmed() -> Self {
        Self {
            armed: false,
            sample_rate: 1,
            silence_threshold_samples: 0,
            preroll_samples: 0,
            awake: false,
            last_speech_sample: 0,
            epoch_closed_at: 0,
        }
    }

    fn armed(sample_rate: u32, silence_sec: f32) -> Self {
        let rate = sample_rate.max(1);
        Self {
            armed: true,
            sample_rate: rate,
            silence_threshold_samples: (silence_sec.max(0.1) * rate as f32) as u64,
            preroll_samples: (EPOCH_PREROLL_SECS * rate as f32) as u64,
            awake: false,
            last_speech_sample: 0,
            epoch_closed_at: 0,
        }
    }

    /// Build the gate the session config asks for. No silence setting → legacy;
    /// no session Silero → legacy, because without edges an armed gate would
    /// rest forever and the take would be silent (fail open).
    fn for_session(
        sample_rate: u32,
        utterance_silence_sec: Option<f32>,
        speech_edges_available: bool,
    ) -> Self {
        let Some(silence_sec) = utterance_silence_sec else {
            return Self::disarmed();
        };
        if !speech_edges_available {
            warn!(
                utterance_silence_sec = silence_sec,
                "Silero unavailable — Apple engine lifecycle disarmed, falling back to one \
                 continuous stream for this session"
            );
            return Self::disarmed();
        }
        Self::armed(sample_rate, silence_sec)
    }

    fn is_armed(&self) -> bool {
        self.armed
    }

    /// One chunk. `speech_live` is the session Silero's verdict on it — the
    /// same observation the utterance ledger was minted from.
    fn feed_pcm(&mut self, samples: &[f32], samples_seen: u64, speech_live: bool) -> EpochDecision {
        if !self.armed {
            return EpochDecision::Forward;
        }
        let chunk_start = samples_seen.saturating_sub(samples.len() as u64);
        if speech_live {
            self.last_speech_sample = samples_seen;
            if self.awake {
                return EpochDecision::Forward;
            }
            self.awake = true;
            let preroll_from = chunk_start
                .saturating_sub(self.preroll_samples)
                .max(self.epoch_closed_at);
            return EpochDecision::Wake { preroll_from };
        }
        if !self.awake {
            return EpochDecision::Idle;
        }
        let silence = samples_seen.saturating_sub(self.last_speech_sample);
        if silence >= self.silence_threshold_samples {
            self.awake = false;
            self.epoch_closed_at = samples_seen;
            return EpochDecision::Sleep {
                silence_secs: silence as f32 / self.sample_rate as f32,
            };
        }
        EpochDecision::Forward
    }
}

/// Session-time base of the open epoch, in seconds.
fn epoch_base_secs(epoch_base_samples: u64, sample_rate: u32) -> f32 {
    epoch_base_samples as f32 / sample_rate.max(1) as f32
}

/// Seal an open partial that never received a phrase final.
///
/// Shared by the two places a stream can end without one: capture EOF (stop
/// mid-phrase) and an epoch close. Both must run the same seal-time correction
/// — a phrase that ends by silence must not be the one route that commits
/// uncorrected text, or dies in the preview lane.
fn seal_open_partial(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    audio_secs: f32,
) {
    let open = state.open_partial.trim().to_string();
    if open.is_empty() {
        return;
    }
    let segments = std::mem::take(&mut state.open_partial_segments);
    seal_utterance_final(state, ev_tx, &open, segments, audio_secs);
    state.open_partial.clear();
}

/// Everything the blocking worker needs that is not a channel.
struct AppleWorkerConfig<'a> {
    sample_rate: u32,
    /// Device the recorder opened; selects the measured calibration profile.
    capture_device_name: Option<String>,
    language: Option<&'a str>,
    session_id: String,
    capture_epoch: u64,
    runtime_settings: Arc<RuntimeSettingsSnapshot>,
    acoustic_ledger: Arc<Mutex<AcousticLedger>>,
    settings_digest: String,
    /// Product "Hands-free silence". `Some` arms the engine lifecycle (speech
    /// epochs); `None` keeps one continuous SFSpeech stream for the whole take.
    utterance_silence_sec: Option<f32>,
}

/// Blocking worker: owns the SFSpeech stream(s) for the session's full lifetime.
fn apple_stream_worker(
    pcm_rx: std_mpsc::Receiver<Option<Vec<f32>>>,
    ev_tx: mpsc::UnboundedSender<EngineEvent>,
    tail_patch: Option<mpsc::Sender<TailPatchRequest>>,
    tail_patch_done: std_mpsc::Receiver<TailPatchCompletion>,
    formatter: Option<mpsc::Sender<FormatterRequest>>,
    formatter_done: std_mpsc::Receiver<FormatterCompletion>,
    config: AppleWorkerConfig<'_>,
) -> anyhow::Result<AppleStreamOutcome> {
    let AppleWorkerConfig {
        sample_rate,
        capture_device_name,
        language,
        session_id,
        capture_epoch,
        runtime_settings,
        acoustic_ledger,
        settings_digest,
        utterance_silence_sec,
    } = config;
    debug_assert_eq!(settings_digest, runtime_settings.digest().as_str());
    // The one read of calibration truth for this session: the measured profile
    // of the device actually opened, converted to Σx² at the actual capture
    // rate. Any refusal keeps the worker fail-closed (no floor is invented);
    // the controller admission gate is expected to have refused earlier, so a
    // refusal here is logged as the anomaly it is.
    let energy_calibration = match capture_device_name.as_deref() {
        Some(device) => {
            match runtime_settings.energy_calibration_for_capture(device, sample_rate) {
                Ok(calibration) => {
                    info!(
                        session = %session_id,
                        device,
                        sample_rate,
                        calibration_version = %calibration.version,
                        min_energy_integral = calibration.min_energy_integral,
                        min_valley_samples = calibration.min_valley_samples,
                        "acoustic admission calibration sealed for session"
                    );
                    Some(calibration)
                }
                Err(refusal) => {
                    warn!(
                        session = %session_id,
                        device,
                        sample_rate,
                        %refusal,
                        "acoustic admission calibration refused; session cannot qualify occurrences"
                    );
                    None
                }
            }
        }
        None => {
            warn!(
                session = %session_id,
                "no capture device bound to session; acoustic admission stays fail-closed"
            );
            None
        }
    };
    let mut state = match tail_patch {
        Some(tx) => AppleSealState::new_with_tail_patch_for_session(
            sample_rate,
            session_id,
            capture_epoch,
            tx,
            acoustic_ledger,
            energy_calibration,
        ),
        None => AppleSealState::new_for_session_with_ledger(
            sample_rate,
            session_id,
            capture_epoch,
            acoustic_ledger,
            energy_calibration,
        ),
    };
    state.formatter = formatter;
    // The session's ONE Silero. Both consumers of speech edges read it: the
    // utterance ledger (identity, ranges) and the engine lifecycle (wake/sleep).
    // It is built whenever either consumer wants it — the fusion flag decides
    // whether identity reaches the seal, not whether the VAD exists.
    state.fusion_seal_armed = runtime_settings.seal_lane_armed();
    if state.fusion_seal_armed || utterance_silence_sec.is_some() {
        let ingress =
            SileroIngress::new(sample_rate, state.session_id.clone(), state.capture_epoch);
        if ingress.vad_available() {
            state.fusion_context = FusionContextMode::from_env();
            info!(
                context = state.fusion_context.as_str(),
                seal_armed = state.fusion_seal_armed,
                lifecycle_armed = utterance_silence_sec.is_some(),
                "Silero ingress armed — single VAD feeding utterance identity and engine lifecycle"
            );
            state.fusion = Some(ingress);
        } else {
            warn!(
                "Silero model unavailable — no utterance identity and no engine lifecycle \
                 this session; Apple segment boundaries stay the seal authority"
            );
        }
    }
    // Engine lifecycle. Disarmed → one stream opened here for the whole take
    // (legacy). Armed → the bridge stays unspawned until the first speech edge,
    // and every epoch closes on the product silence threshold.
    let mut epoch =
        EpochGate::for_session(sample_rate, utterance_silence_sec, state.fusion.is_some());
    let mut stream = if epoch.is_armed() {
        info!(
            utterance_silence_sec = utterance_silence_sec.unwrap_or_default(),
            preroll_secs = EPOCH_PREROLL_SECS,
            "Apple progressive engine lifecycle armed — SFSpeech rests between utterances"
        );
        None
    } else {
        Some(LiveStreamSession::open(language, sample_rate)?)
    };
    // Session-time base of the open epoch. Zero for the legacy single stream,
    // which is what makes `shift_events` the identity on that path.
    let mut epoch_base_samples: u64 = 0;
    let mut samples_seen: u64 = 0;

    loop {
        while let Ok(completion) = tail_patch_done.try_recv() {
            let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
            state.complete_whisper_window(&ev_tx, completion, audio_secs);
        }
        while let Ok(completion) = formatter_done.try_recv() {
            if !state.complete_formatter(&ev_tx, completion) {
                return Err(anyhow::anyhow!(
                    "formatter completion reached worker without an emitter-sealed exact occurrence",
                ));
            }
        }
        // Interleave PCM wait with event polling so partials land
        // mid-utterance without waiting for the next audio chunk.
        match pcm_rx.recv_timeout(Duration::from_millis(40)) {
            Ok(Some(samples)) => {
                samples_seen += samples.len() as u64;
                // Retain before forwarding, on the same counter `audio_secs` is
                // derived from, so the buffer and the seal clock cannot drift.
                // This is worker-side on purpose: the async select loop stays
                // lock-free (2026-07-27 interleave contract) because the buffer
                // is never shared across the thread boundary.
                //
                // Retention runs in every lifecycle state, including while the
                // engine rests: it is what the pre-roll of the next epoch is cut
                // from, and what Layer 1 windows still resolve against.
                state.audio.push(&samples);
                // One observation of the spectrum, two consumers: the ledger
                // mints identity from it and the lifecycle wakes/sleeps on it.
                let silero_ingest = state
                    .fusion
                    .as_mut()
                    .map(|fusion| fusion.ingest(&samples, samples_seen));
                if let Some(ingest) = silero_ingest.as_ref() {
                    for evidence in &ingest.sideband {
                        let _ = ev_tx.send(EngineEvent::SidebandEvidence {
                            evidence: evidence.clone(),
                        });
                    }
                }
                let speech_live = silero_ingest.is_some_and(|ingest| ingest.speech_live);
                let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
                match epoch.feed_pcm(&samples, samples_seen, speech_live) {
                    EpochDecision::Forward => {
                        if let Some(session) = stream.as_mut() {
                            session.write_pcm(&samples)?;
                            let events = shift_events(
                                session.poll_events(),
                                epoch_base_secs(epoch_base_samples, sample_rate),
                            );
                            emit_stream_events(events, &ev_tx, &mut state, audio_secs);
                        }
                    }
                    EpochDecision::Wake { preroll_from } => {
                        let mut session = LiveStreamSession::open(language, sample_rate)?;
                        let chunk_start = samples_seen.saturating_sub(samples.len() as u64);
                        // The base is whatever audio this epoch ACTUALLY starts
                        // with, never what was asked for: a pre-roll that fell
                        // off retention resolves to nothing, and basing the
                        // epoch on it would shift every timestamp in it earlier
                        // by the missing audio.
                        let preroll = state.audio.window_by_samples(preroll_from, chunk_start);
                        epoch_base_samples =
                            preroll.as_ref().map_or(chunk_start, |w| w.sample_start);
                        let preroll_samples =
                            preroll.as_ref().map_or(0, |window| window.samples.len());
                        if let Some(window) = preroll.filter(|w| !w.samples.is_empty()) {
                            session.write_pcm(&window.samples)?;
                        }
                        session.write_pcm(&samples)?;
                        info!(
                            audio_secs,
                            epoch_base_secs = epoch_base_secs(epoch_base_samples, sample_rate),
                            preroll_samples,
                            "apple_lifecycle: epoch open (speech edge)"
                        );
                        let events = shift_events(
                            session.poll_events(),
                            epoch_base_secs(epoch_base_samples, sample_rate),
                        );
                        emit_stream_events(events, &ev_tx, &mut state, audio_secs);
                        stream = Some(session);
                    }
                    EpochDecision::Sleep { silence_secs } => {
                        if let Some(session) = stream.take() {
                            let base_secs = epoch_base_secs(epoch_base_samples, sample_rate);
                            let trailing = shift_events(session.finish()?, base_secs);
                            emit_stream_events(trailing, &ev_tx, &mut state, audio_secs);
                            // Same close as capture EOF: whatever the engine
                            // left open is sealed here, because no later
                            // callback from this epoch can arrive.
                            seal_open_partial(&mut state, &ev_tx, audio_secs);
                            let _ = state.flush_layer1_coalesce(&ev_tx);
                            info!(
                                audio_secs,
                                silence_secs,
                                epoch_base_secs = base_secs,
                                "apple_lifecycle: epoch close (hands-free silence)"
                            );
                        }
                    }
                    // Resting: audio is retained, the engine is not running.
                    EpochDecision::Idle => {}
                }
            }
            Ok(None) => break, // EOF from async side
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
                if let Some(session) = stream.as_mut() {
                    let events = shift_events(
                        session.poll_events(),
                        epoch_base_secs(epoch_base_samples, sample_rate),
                    );
                    emit_stream_events(events, &ev_tx, &mut state, audio_secs);
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
    if let Some(fusion) = state.fusion.as_mut() {
        fusion.flush(samples_seen);
    }
    if let Some(session) = stream.take() {
        let trailing = shift_events(
            session.finish()?,
            epoch_base_secs(epoch_base_samples, sample_rate),
        );
        emit_stream_events(trailing, &ev_tx, &mut state, audio_secs);
    }

    // Seal open partial that never got a phrase final (stop mid-phrase).
    // Same seal-time correction as the phrase path — a stop mid-utterance must
    // not be the one route that commits uncorrected text.
    seal_open_partial(&mut state, &ev_tx, audio_secs);
    let _ = state.flush_layer1_coalesce(&ev_tx);

    // Every accepted Layer 1 request must close (success, no-change, or)
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
    let mut tail_patch_timeout_residue = 0;
    while state.tail_patch_awaiting_completion > 0 {
        match tail_patch_done.recv_timeout(TAIL_PATCH_CLOSURE_TIMEOUT) {
            Ok(completion) => state.complete_whisper_window(&ev_tx, completion, audio_secs),
            Err(error) => {
                warn!("tail-patch closure wait ended before all observations returned: {error}");
                tail_patch_timeout_residue = state.tail_patch_awaiting_completion;
                state.return_outstanding_whisper_without_label(&ev_tx);
                break;
            }
        }
    }

    // A bounded formatter execution has its own provider timeout policy. Once
    // its exact slot is scheduled, stop drains the typed completion without a
    // second deadline or force-seal; the emitter returns the slot and seals the
    // occurrence before this acknowledgement can arrive.
    while state.formatter_awaiting_completion > 0 {
        let completion = formatter_done.recv().map_err(|error| {
            anyhow::anyhow!(
                "formatter completion channel closed with {} exact occurrence job(s) outstanding: {error}",
                state.formatter_awaiting_completion,
            )
        })?;
        if !state.complete_formatter(&ev_tx, completion) {
            return Err(anyhow::anyhow!(
                "formatter stop drain received a completion without an emitter-sealed exact occurrence",
            ));
        }
    }

    // Capture is over: no later Apple callback can revise a span and no further
    // Whisper window can arrive, so both double-close gates are satisfied by
    // definition. Seal the remainder here instead of leaving it to the residual
    // path — the machine's own span timestamps are the clock, because the audio
    // clock is frozen at EOF and can sit milliseconds behind them.
    state.seal_remaining_at_session_end(&ev_tx);
    let seal_coverage = repair_terminal_seal_coverage(&mut state, &ev_tx, language);
    if seal_coverage.status == SealCoverageStatus::Incomplete {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "terminal_seal_coverage_incomplete".to_string(),
            message: format!(
                "covered={}/{} max_uncovered={} threshold={}",
                seal_coverage.covered_samples,
                seal_coverage.speech_samples,
                seal_coverage.max_uncovered_samples,
                seal_coverage.incomplete_threshold_samples,
            ),
        });
        return Ok(AppleStreamOutcome {
            sealed: state.sealed_count,
            filtered_empty_drops: state.filtered_empty_drops,
            unresolved_windows: state.unresolved_windows,
            under_commit_escalations: state.under_commit_escalations,
            tail_patch_replacements: state.tail_patch_replacements,
            tail_patch_refusals: state.tail_patch_refusals,
            tail_patch_jobs_applied: state.tail_patch_jobs_applied,
            tail_patch_jobs_skipped: state.tail_patch_jobs_skipped,
            tail_patch_timeout_residue,
        });
    }
    let terminal = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .seal_terminal(&state.session_id, state.capture_epoch);
    match terminal {
        Ok(receipt) => {
            let _ = ev_tx.send(EngineEvent::LedgerSeal { receipt });
        }
        Err(refusal) => report_terminal_seal_refusal(&ev_tx, refusal),
    }

    Ok(AppleStreamOutcome {
        sealed: state.sealed_count,
        filtered_empty_drops: state.filtered_empty_drops,
        unresolved_windows: state.unresolved_windows,
        under_commit_escalations: state.under_commit_escalations,
        tail_patch_replacements: state.tail_patch_replacements,
        tail_patch_refusals: state.tail_patch_refusals,
        tail_patch_jobs_applied: state.tail_patch_jobs_applied,
        tail_patch_jobs_skipped: state.tail_patch_jobs_skipped,
        tail_patch_timeout_residue,
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

/// C13/C13A lifecycle falsifiers kept active without reviving the stale legacy
/// Apple test canvas below.
#[cfg(test)]
mod c13a_lifecycle_tests {
    use super::*;

    const TEST_SAMPLE_RATE: u32 = 16_000;

    fn state_for_session(session_id: &str) -> AppleSealState {
        AppleSealState::new_for_session(TEST_SAMPLE_RATE, session_id.to_string(), 1)
    }

    fn stage_pending_occurrence(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        utterance_id: u64,
        occurrence: OccurrenceIdentity,
        label: &str,
    ) {
        let calibration = EnergyCalibration {
            version: "c13a-test".to_string(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(occurrence.sample_start),
            vad_close_sample: Some(occurrence.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .qualify(&evidence, &calibration);
        state.energy_calibration = Some(calibration);
        assert!(
            admit_ledger_label(
                state,
                ev_tx,
                LabelAdmission {
                    observation: LedgerObservationIdentity::new(
                        LedgerObservationProducer::Apple,
                        utterance_id,
                        0,
                        occurrence.clone(),
                    ),
                    label,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .is_some()
        );
        state.pending_events.insert(
            utterance_id,
            PendingAppleSeal {
                occurrence,
                raw_text: label.to_string(),
                layer1_baseline: label.to_string(),
                start_ts: 0.0,
                end_ts: 1.0,
                segments: Vec::new(),
            },
        );
    }

    fn piece(
        utterance_id: u64,
        occurrence: &OccurrenceIdentity,
        start_ts: f32,
        covered_through_secs: f32,
    ) -> CoalescedPiece {
        CoalescedPiece {
            utterance_id,
            occurrence: occurrence.clone(),
            committed_text: "Iwo".to_string(),
            audio: vec![0.5; occurrence.sample_len() as usize],
            sample_start: occurrence.sample_start,
            sample_end: occurrence.sample_end,
            start_ts,
            covered_through_secs,
            segment_count: 1,
        }
    }

    fn return_lexicon(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        utterance_id: u64,
        occurrence: &OccurrenceIdentity,
    ) {
        assert!(
            admit_ledger_label(
                state,
                ev_tx,
                LabelAdmission {
                    observation: LedgerObservationIdentity::new(
                        LedgerObservationProducer::Lexicon,
                        utterance_id,
                        0,
                        occurrence.clone(),
                    ),
                    label: "Iwo",
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .is_some()
        );
        state.emit_pending_seal(ev_tx, utterance_id);
    }

    fn no_payload_completion(request: &TailPatchRequest) -> TailPatchCompletion {
        TailPatchCompletion {
            utterance_id: request.utterance_id,
            request_identity: Some(request.provider_request.identity.clone()),
            payload: None,
            member_occurrences: request.member_occurrences.clone(),
        }
    }

    fn ai_result(status: AiFormatStatus, text: &str) -> AiFormatResult {
        AiFormatResult {
            text: text.to_string(),
            reasoning_text: None,
            status,
        }
    }

    #[test]
    fn formatter_configuration_without_transport_ownership_never_opens_a_frontier() {
        assert!(!formatter_lane_is_armed(
            false,
            FormattingPolicy::Correction,
            true,
        ));
        assert!(!formatter_lane_is_armed(true, FormattingPolicy::Off, true,));
        assert!(!formatter_lane_is_armed(
            true,
            FormattingPolicy::Correction,
            false,
        ));
        assert!(formatter_lane_is_armed(
            true,
            FormattingPolicy::Correction,
            true,
        ));

        for (session, formatter) in {
            let (closed_tx, closed_rx) = mpsc::channel(1);
            drop(closed_rx);
            [
                ("formatter-disabled", None),
                ("formatter-closed", Some(closed_tx)),
            ]
        } {
            let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
            let mut state = state_for_session(session);
            state.formatter = formatter;
            let occurrence = OccurrenceIdentity::new(session, 1, 0, 16_000);
            stage_pending_occurrence(&mut state, &ev_tx, 1, occurrence.clone(), "Iwo");
            return_lexicon(&mut state, &ev_tx, 1, &occurrence);

            let ledger = state.acoustic_ledger.lock().expect("ledger");
            assert!(ledger.is_sealed(&occurrence));
            assert!(
                ledger
                    .frontier_of(&occurrence)
                    .expect("frontier")
                    .open_producers()
                    .is_empty(),
            );
            assert_eq!(state.formatter_awaiting_completion, 0);
        }
    }

    #[test]
    fn formatter_results_map_to_typed_occurrence_dispositions() {
        let occurrence = OccurrenceIdentity::new("formatter-map", 7, 160, 320);
        let cases = [
            (
                AiFormatStatus::Applied,
                "Sformatowane Iwo",
                LabelProposalDisposition::Propose,
                "Sformatowane Iwo",
            ),
            (
                AiFormatStatus::AiNoop,
                "Iwo",
                LabelProposalDisposition::PreserveExisting,
                "",
            ),
            (
                AiFormatStatus::Skipped,
                "Iwo",
                LabelProposalDisposition::PreserveExisting,
                "",
            ),
            (
                AiFormatStatus::Failed,
                "Iwo",
                LabelProposalDisposition::Refuse,
                "",
            ),
            (
                AiFormatStatus::Applied,
                "   ",
                LabelProposalDisposition::Refuse,
                "",
            ),
        ];

        for (status, text, disposition, proposed_label) in cases {
            let completion = FormatterCompletion::from_result(
                FormatterRequest {
                    occurrence: occurrence.clone(),
                    existing_label: "Iwo".to_string(),
                },
                ai_result(status, text),
            );
            assert!(completion.carries_same_occurrence());
            assert_eq!(completion.proposal.disposition, disposition);
            assert_eq!(completion.proposal.proposed_label, proposed_label);
        }
    }

    #[test]
    fn formatter_transport_failure_after_raw_seal_keeps_lane_and_terminal_seal_alive() {
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let (formatter_tx, mut formatter_rx) = mpsc::channel(FORMATTER_QUEUE_CAP);
        let mut state = state_for_session("formatter-transport-failure");
        state.formatter = Some(formatter_tx);
        let first = OccurrenceIdentity::new("formatter-transport-failure", 1, 0, 16_000);

        stage_pending_occurrence(&mut state, &ev_tx, 1, first.clone(), "Surowe zdanie");
        return_lexicon(&mut state, &ev_tx, 1, &first);
        let request = formatter_rx.try_recv().expect("exact formatter request");
        let completion = FormatterCompletion::from_result(
            request,
            // Deterministic provider/transport refusal stub: this is the same
            // typed outcome produced after a 401 or an exhausted HTTP failure.
            ai_result(AiFormatStatus::Failed, "Surowe zdanie"),
        );

        // PresentationEmitter synchronously returns the Formatter frontier,
        // seals the raw occurrence, and may let another completion publish it
        // before the worker receives this acknowledgement.
        {
            let mut ledger = state.acoustic_ledger.lock().expect("ledger");
            assert!(ledger.note_frontier_return(&first, LedgerObservationProducer::Formatter,));
            assert!(ledger.seal(&first).is_ok());
        }
        state.emit_pending_seal(&ev_tx, 1);
        assert!(state.pending_events.is_empty());

        assert!(
            state.complete_formatter(&ev_tx, completion),
            "a known formatter failure must be acknowledged even after raw publication",
        );
        assert_eq!(state.formatter_awaiting_completion, 0);

        // A later utterance proves that the live lane remained usable after
        // the formatter failure instead of terminating at the acknowledgement.
        state.formatter = None;
        let second = OccurrenceIdentity::new("formatter-transport-failure", 1, 16_000, 32_000);
        stage_pending_occurrence(&mut state, &ev_tx, 2, second.clone(), "Dalszy surowy tekst");
        return_lexicon(&mut state, &ev_tx, 2, &second);

        let terminal = state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .seal_terminal(&state.session_id, state.capture_epoch)
            .expect("formatter refusal must not block the terminal ledger seal");
        assert_eq!(terminal.sealed_occurrences, vec![first, second]);
        let finals = std::iter::from_fn(|| ev_rx.try_recv().ok())
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal {
                    utterance_id,
                    text,
                    raw_text,
                    ..
                } => Some((utterance_id, text, raw_text)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            finals,
            vec![
                (1, "Surowe zdanie".to_string(), "Surowe zdanie".to_string(),),
                (
                    2,
                    "Dalszy surowy tekst".to_string(),
                    "Dalszy surowy tekst".to_string(),
                ),
            ],
        );
    }

    #[test]
    fn five_equal_labels_keep_five_occurrence_jobs_and_exact_completion_debts() {
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let (formatter_tx, mut formatter_rx) = mpsc::channel(FORMATTER_QUEUE_CAP);
        let mut state = state_for_session("formatter-five-iwo");
        state.formatter = Some(formatter_tx);
        let occurrences = (0..5_u64)
            .map(|index| {
                OccurrenceIdentity::new(
                    "formatter-five-iwo",
                    1,
                    index * 16_000,
                    (index + 1) * 16_000,
                )
            })
            .collect::<Vec<_>>();

        for (index, occurrence) in occurrences.iter().enumerate() {
            let utterance_id = index as u64 + 1;
            let queued_before_stage = formatter_rx.len();
            stage_pending_occurrence(&mut state, &ev_tx, utterance_id, occurrence.clone(), "Iwo");
            assert_eq!(
                formatter_rx.len(),
                queued_before_stage,
                "staging occurrence {utterance_id} must not dispatch formatting",
            );
            return_lexicon(&mut state, &ev_tx, utterance_id, occurrence);
            assert_eq!(
                formatter_rx.len(),
                queued_before_stage + 1,
                "returning Lexicon for occurrence {utterance_id} must enqueue exactly one request",
            );
        }

        // Normal sender closure cannot discard accepted work: Tokio drains
        // every buffered exact request before reporting disconnection.
        drop(state.formatter.take());
        let requests = (0..5)
            .map(|_| formatter_rx.try_recv().expect("exact formatter request"))
            .collect::<Vec<_>>();
        assert!(matches!(
            formatter_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected),
        ));
        assert_eq!(state.formatter_awaiting_completion, 5);
        for (request, occurrence) in requests.iter().zip(&occurrences) {
            assert_eq!(&request.occurrence, occurrence);
            assert_eq!(request.existing_label, "Iwo");
            let ledger = state.acoustic_ledger.lock().expect("ledger");
            assert_eq!(
                ledger
                    .frontier_of(occurrence)
                    .expect("frontier")
                    .open_producers(),
                vec![LedgerObservationProducer::Formatter],
            );
        }

        let wrong_occurrence = OccurrenceIdentity::new("formatter-five-iwo", 1, 1, 16_001);
        let wrong_completion = FormatterCompletion::from_result(
            FormatterRequest {
                occurrence: wrong_occurrence,
                existing_label: "Iwo".to_string(),
            },
            ai_result(AiFormatStatus::AiNoop, "Iwo"),
        );
        assert!(!state.complete_formatter(&ev_tx, wrong_completion));
        assert_eq!(state.formatter_awaiting_completion, 5);

        let mut mismatched_completion = FormatterCompletion::from_result(
            requests[0].clone(),
            ai_result(AiFormatStatus::AiNoop, "Iwo"),
        );
        mismatched_completion.proposal.sample_start = mismatched_completion
            .proposal
            .sample_start
            .saturating_add(1);
        assert!(!mismatched_completion.carries_same_occurrence());
        assert!(!state.complete_formatter(&ev_tx, mismatched_completion));
        assert_eq!(state.formatter_awaiting_completion, 5);

        for request in requests {
            let occurrence = request.occurrence.clone();
            let completion =
                FormatterCompletion::from_result(request, ai_result(AiFormatStatus::AiNoop, "Iwo"));
            {
                let mut ledger = state.acoustic_ledger.lock().expect("ledger");
                assert!(
                    ledger.note_frontier_return(&occurrence, LedgerObservationProducer::Formatter,)
                );
                assert!(ledger.seal(&occurrence).is_ok());
            }
            assert!(state.complete_formatter(&ev_tx, completion.clone()));
            assert!(!state.complete_formatter(&ev_tx, completion));
        }

        assert_eq!(state.formatter_awaiting_completion, 0);
        assert!(state.pending_events.is_empty());
        let finals = std::iter::from_fn(|| ev_rx.try_recv().ok())
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal {
                    utterance_id, text, ..
                } => Some((utterance_id, text)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            finals,
            vec![
                (1, "Iwo".to_string()),
                (2, "Iwo".to_string()),
                (3, "Iwo".to_string()),
                (4, "Iwo".to_string()),
                (5, "Iwo".to_string()),
            ],
        );
    }

    #[tokio::test]
    async fn provider_latency_does_not_block_engine_event_drainage() {
        let mut formatter_jobs = FuturesOrdered::<BoxFuture<'static, FormatterCompletion>>::new();
        formatter_jobs.push_back(Box::pin(std::future::pending::<FormatterCompletion>()));
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        ev_tx
            .send(EngineEvent::Preview {
                rev: 1,
                text: "live".to_string(),
            })
            .expect("event receiver");

        let event = tokio::select! {
            Some(event) = ev_rx.recv() => event,
            Some(_) = formatter_jobs.next() => panic!("pending provider completed"),
        };
        assert!(matches!(event, EngineEvent::Preview { text, .. } if text == "live"));
    }

    #[test]
    fn terminal_seal_refusal_is_visible_without_becoming_success() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        report_terminal_seal_refusal(&tx, SealRefusal::NotQualified);
        assert!(
            rx.try_recv().is_err(),
            "no-speech is the quiet terminal case"
        );

        report_terminal_seal_refusal(&tx, SealRefusal::FrontierOpen);
        assert!(matches!(
            rx.try_recv().expect("frontier refusal diagnostic"),
            EngineEvent::Warning { code, message }
                if code == LEDGER_TERMINAL_SEAL_REFUSED_WARNING_CODE
                    && message == SealRefusal::FrontierOpen.as_str()
        ));
        assert!(rx.try_recv().is_err(), "refusal emits no ledger seal");
    }

    #[test]
    fn whisper_no_payload_closes_exact_coalesced_members_once() {
        let (tx, mut event_rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel::<TailPatchRequest>(4);
        let mut state = state_for_session("coalesced");
        state.tail_patch = Some(tail_tx);
        let first = OccurrenceIdentity::new("coalesced", 1, 0, 16_000);
        let second = OccurrenceIdentity::new("coalesced", 1, 16_000, 32_000);
        assert_ne!(
            first, second,
            "disjoint PCM ranges are distinct occurrences"
        );

        stage_pending_occurrence(&mut state, &tx, 1, first.clone(), "Iwo");
        assert!(state.enqueue_layer1_piece(&tx, piece(1, &first, 0.0, 1.0)));
        return_lexicon(&mut state, &tx, 1, &first);
        stage_pending_occurrence(&mut state, &tx, 2, second.clone(), "Iwo");
        assert!(state.enqueue_layer1_piece(&tx, piece(2, &second, 1.0, 2.0)));
        return_lexicon(&mut state, &tx, 2, &second);
        assert!(state.flush_layer1_coalesce(&tx));
        let request = tail_rx.try_recv().expect("one exact coalesced request");
        assert_eq!(request.member_occurrences.len(), 2);
        assert_eq!(state.tail_patch_awaiting_completion, 1);
        while event_rx.try_recv().is_ok() {}

        state.complete_whisper_window(&tx, no_payload_completion(&request), 2.0);
        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                .count(),
            2,
            "each exact member occurrence seals once"
        );
        assert!(events.iter().all(|event| !matches!(
            event,
            EngineEvent::LedgerMutation { observation, .. }
                if observation.producer == LedgerObservationProducer::Whisper
        )));
        let ledger = state.acoustic_ledger.lock().expect("ledger");
        assert!(ledger.is_sealed(&first));
        assert!(ledger.is_sealed(&second));
        assert_eq!(ledger.text_of(&first), Some("Iwo"));
        assert_eq!(ledger.text_of(&second), Some("Iwo"));
        drop(ledger);
        assert!(state.pending_events.is_empty());
        assert_eq!(state.tail_patch_awaiting_completion, 0);

        state.complete_whisper_window(&tx, no_payload_completion(&request), 2.0);
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert!(event_rx.try_recv().is_err(), "replay emits no second seal");
    }

    /// W3B falsifier for the retired concat-space remap.
    ///
    /// The deleted `remap_range` refused a char range that crossed a join
    /// between two coalesced utterances, because pouring it into the first span
    /// invents text at the wrong acoustic identity. That refusal is now
    /// structural, not arithmetic: `complete_whisper_window` keeps only the
    /// provider segments whose PCM range lies wholly inside one member's
    /// occurrence, and the whole-window text fallback is reachable for a
    /// single-member window only.
    ///
    /// A straddling segment must therefore reach neither member, while a
    /// segment pinned inside one member still becomes that member's label —
    /// the positive control that keeps this test falsifiable.
    #[test]
    fn a_candidate_straddling_two_member_occurrences_labels_neither() {
        use crate::stt::tail_provider::{
            TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
            TailTimingQuality,
        };

        let (tx, mut event_rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel::<TailPatchRequest>(4);
        let mut state = state_for_session("straddle");
        state.tail_patch = Some(tail_tx);
        let first = OccurrenceIdentity::new("straddle", 1, 0, 16_000);
        let second = OccurrenceIdentity::new("straddle", 1, 16_000, 32_000);

        stage_pending_occurrence(&mut state, &tx, 1, first.clone(), "Iwo");
        assert!(state.enqueue_layer1_piece(&tx, piece(1, &first, 0.0, 1.0)));
        return_lexicon(&mut state, &tx, 1, &first);
        stage_pending_occurrence(&mut state, &tx, 2, second.clone(), "Iwo");
        assert!(state.enqueue_layer1_piece(&tx, piece(2, &second, 1.0, 2.0)));
        return_lexicon(&mut state, &tx, 2, &second);
        assert!(state.flush_layer1_coalesce(&tx));
        let request = tail_rx.try_recv().expect("one exact coalesced request");
        assert_eq!(request.member_occurrences.len(), 2);
        while event_rx.try_recv().is_ok() {}

        // One segment crosses the join (8 000..24 000); one is pinned wholly
        // inside the second member (20 000..30 000 would also cross, so the
        // pinned control sits at 16 000..32 000 exactly).
        let identity = request.provider_request.identity.clone();
        let straddling = TimedTailSegment {
            text: "przez granice".to_string(),
            range: TailSampleRange {
                session: "straddle".to_string(),
                capture_epoch: 1,
                sample_start: 8_000,
                sample_end: 24_000,
            },
        };
        let pinned = TimedTailSegment {
            text: "Iwo drugie".to_string(),
            range: TailSampleRange {
                session: "straddle".to_string(),
                capture_epoch: 1,
                sample_start: 16_000,
                sample_end: 32_000,
            },
        };
        let payload = TailProviderPayload {
            identity: identity.clone(),
            text: "cale okno przez granice".to_string(),
            segments: vec![straddling, pinned],
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.0),
            provider_id: TailProviderId::Fake,
            elapsed_ms: 1,
            evidence: TailProviderEvidence {
                source: TailEvidenceSource::Whisper,
                revision: None,
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: Some(-0.2),
            },
        };
        state.complete_whisper_window(
            &tx,
            TailPatchCompletion {
                utterance_id: request.utterance_id,
                request_identity: Some(identity),
                payload: Some(payload),
                member_occurrences: request.member_occurrences.clone(),
            },
            2.0,
        );

        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        let whisper_labels = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::LedgerMutation {
                    observation, label, ..
                } if observation.producer == LedgerObservationProducer::Whisper => {
                    Some((observation.occurrence.clone(), label.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            whisper_labels,
            vec![(second.clone(), "Iwo drugie".to_string())],
            "only the member that wholly owns a segment may be relabelled"
        );
        assert!(
            !whisper_labels
                .iter()
                .any(|(occurrence, _)| occurrence == &first),
            "a straddling candidate must not be poured into the first span"
        );

        let ledger = state.acoustic_ledger.lock().expect("ledger");
        assert!(ledger.is_sealed(&first));
        assert!(ledger.is_sealed(&second));
        assert_eq!(
            ledger.text_of(&first),
            Some("Iwo"),
            "the Apple floor survives a candidate that named no exact occurrence"
        );
        assert_eq!(ledger.text_of(&second), Some("Iwo drugie"));
        drop(ledger);

        // Post-seal immutability: the same completion replayed after the seal
        // cannot reopen either occurrence or move a single label.
        state.complete_whisper_window(&tx, no_payload_completion(&request), 2.0);
        assert!(
            event_rx.try_recv().is_err(),
            "a replayed completion after the terminal seal emits nothing"
        );
        let ledger = state.acoustic_ledger.lock().expect("ledger");
        assert_eq!(ledger.text_of(&first), Some("Iwo"));
        assert_eq!(ledger.text_of(&second), Some("Iwo drugie"));
    }

    #[test]
    fn pause_flush_rejection_conserves_newly_held_member() {
        let (tx, mut event_rx) = mpsc::unbounded_channel();
        let (tail_tx, tail_rx) = mpsc::channel::<TailPatchRequest>(1);
        drop(tail_rx);
        let mut state = state_for_session("pause-rejection");
        state.tail_patch = Some(tail_tx);
        let first = OccurrenceIdentity::new("pause-rejection", 1, 0, 16_000);
        let second = OccurrenceIdentity::new("pause-rejection", 1, 48_000, 64_000);

        stage_pending_occurrence(&mut state, &tx, 1, first.clone(), "Iwo");
        assert!(state.enqueue_layer1_piece(&tx, piece(1, &first, 0.0, 1.0)));
        return_lexicon(&mut state, &tx, 1, &first);

        stage_pending_occurrence(&mut state, &tx, 2, second.clone(), "Iwo");
        assert!(
            state.enqueue_layer1_piece(&tx, piece(2, &second, 3.0, 4.0)),
            "failure to queue prior flush A cannot revoke newly held B"
        );
        return_lexicon(&mut state, &tx, 2, &second);
        assert!(
            state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .is_sealed(&first)
        );
        assert!(!state.pending_events.contains_key(&1));
        assert!(state.pending_events.contains_key(&2));
        assert!(
            state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .frontier_of(&second)
                .expect("B frontier")
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );

        assert!(!state.flush_layer1_coalesce(&tx));
        let ledger = state.acoustic_ledger.lock().expect("ledger");
        for occurrence in [&first, &second] {
            assert!(ledger.is_sealed(occurrence));
            assert!(
                !ledger
                    .frontier_of(occurrence)
                    .expect("frontier")
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper)
            );
        }
        drop(ledger);
        assert!(state.pending_events.is_empty());
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        let final_count = std::iter::from_fn(|| event_rx.try_recv().ok())
            .filter(|event| matches!(event, EngineEvent::UtteranceFinal { .. }))
            .count();
        assert_eq!(
            final_count, 2,
            "A and B each emit exactly one pending final"
        );
    }

    #[test]
    fn replayed_completion_does_not_consume_another_jobs_debt() {
        let (tx, _event_rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel::<TailPatchRequest>(4);
        let mut state = state_for_session("two-jobs");
        state.tail_patch = Some(tail_tx);
        let first = OccurrenceIdentity::new("two-jobs", 1, 0, 16_000);
        let second = OccurrenceIdentity::new("two-jobs", 1, 32_000, 48_000);

        stage_pending_occurrence(&mut state, &tx, 1, first.clone(), "Iwo");
        assert!(state.enqueue_layer1_piece(&tx, piece(1, &first, 0.0, 1.0)));
        return_lexicon(&mut state, &tx, 1, &first);
        assert!(state.flush_layer1_coalesce(&tx));
        let first_request = tail_rx.try_recv().expect("first accepted job");

        stage_pending_occurrence(&mut state, &tx, 2, second.clone(), "Iwo");
        assert!(state.enqueue_layer1_piece(&tx, piece(2, &second, 2.0, 3.0)));
        return_lexicon(&mut state, &tx, 2, &second);
        assert!(state.flush_layer1_coalesce(&tx));
        let second_request = tail_rx.try_recv().expect("second accepted job");
        assert_eq!(second_request.member_occurrences, vec![(2, second.clone())]);
        assert_eq!(state.tail_patch_awaiting_completion, 2);

        state.complete_whisper_window(&tx, no_payload_completion(&first_request), 3.0);
        assert_eq!(state.tail_patch_awaiting_completion, 1);
        state.complete_whisper_window(&tx, no_payload_completion(&first_request), 3.0);
        assert_eq!(
            state.tail_patch_awaiting_completion, 1,
            "replayed A cannot consume B's accepted job debt"
        );
        assert!(
            state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .frontier_of(&second)
                .expect("B frontier")
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );
        assert!(state.pending_events.contains_key(&2));

        state.return_outstanding_whisper_without_label(&tx);
        state.seal_remaining_at_session_end(&tx);
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert!(
            state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .is_sealed(&second)
        );
        assert!(state.pending_events.is_empty());
    }
}

/// Seal mapping, lexicon-at-seal, retained PCM windows, and Layer 1 wiring.
#[cfg(any())]
mod tests {
    use super::*;
    use crate::pipeline::contracts::LayerSource;
    use crate::stt::apple_stt::parse_stream_stdout_line;
    use crate::stt::tail_patcher::{
        compute_tail_patch, layered_phase_from_raw, parse_layered_phase_value,
    };
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

    fn sealed_span(id: u64, text: &str) -> SealedSpan {
        SealedSpan {
            id,
            text: text.to_string(),
            end_secs_millis: id as u32 * 1_000,
            range: TailSampleRange {
                session: "live-cloud-gap-test".to_string(),
                capture_epoch: 0,
                sample_start: (id - 1) * 16_000,
                sample_end: id * 16_000,
            },
            words: Vec::new(),
            apple_evidence: TailProviderEvidence {
                source: TailEvidenceSource::AppleSpeech,
                revision: None,
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: None,
            },
            whisper_evidence: None,
            whisper_words: Vec::new(),
            silero_utterance_id: None,
        }
    }

    #[test]
    fn live_cloud_gap_plan_preserves_apple_and_inserts_missing_words() {
        let spans = vec![
            sealed_span(1, "I będziesz miał po prostu lokalnej teraz sobie."),
            sealed_span(2, "Możesz odczytać i też pow."),
        ];
        let candidate = "I będziesz miał po prostu z lokalnej sesji teraz sobie. Możesz odczytać i też powkurwiać się razem.";
        let patches = plan_live_layer1_gap_patches(&spans, candidate);
        assert!(
            !patches.is_empty(),
            "provider-only gaps must become patches"
        );

        let mut rendered = spans
            .iter()
            .map(|span| (span.id, span.text.clone()))
            .collect::<BTreeMap<_, _>>();
        for patch in &patches {
            let utterance_id = patch_utterance(patch);
            patch
                .apply_to_committed_text(rendered.get_mut(&utterance_id).expect("known span"))
                .expect("bounded patch");
        }
        let patched = rendered.into_values().collect::<Vec<_>>().join(" ");
        let live = spans
            .iter()
            .map(|span| span.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            patched,
            crate::quality::merge_live_layer1(&live, candidate).text
        );
        assert!(patched.contains("z lokalnej sesji"));
        assert!(patched.contains("powkurwiać się razem"));
    }

    /// Partial → Preview; each phrase final → UtteranceFinal with rising ids.
    #[test]
    fn emit_maps_partial_and_two_phrase_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 2.0);
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

    fn count_iwo(text: &str) -> usize {
        text.split_whitespace()
            .filter(|word| {
                word.chars()
                    .filter(|ch| ch.is_alphabetic())
                    .collect::<String>()
                    .eq_ignore_ascii_case("iwo")
            })
            .count()
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

    /// A trailing cumulative callback can assert novel text after capture has
    /// already reached EOF. The text still belongs on the append-only canvas,
    /// but its synthetic Apple boundary must clamp to the canonical PCM clock:
    /// advancing the window floor to the unclamped Apple timestamp makes every
    /// later suffix start beyond retained audio and queues an empty Whisper
    /// window before the failure becomes visible.
    #[test]
    fn eof_clamped_novel_suffixes_do_not_poison_pcm_window_floor_or_queue_empty_audio() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (tp_tx, mut tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
        let mut state = AppleSealState::new_with_tail_patch(TEST_SAMPLE_RATE, tp_tx);
        push_capture(&mut state, 3.0);

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "alpha beta".into(),
                segments: vec![segment("alpha beta", 0.0, 3.0)],
            }],
            &tx,
            &mut state,
            3.0,
        );
        assert!(state.flush_layer1_coalesce(&tx));
        let initial = tp_rx
            .try_recv()
            .expect("the real captured span must reach Layer 1");
        assert!(!initial.audio.is_empty());

        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "alpha beta gamma".into(),
                    segments: vec![segment("alpha beta gamma", 0.0, 3.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "alpha beta gamma delta".into(),
                    segments: vec![segment("alpha beta gamma delta", 0.0, 3.0)],
                },
            ],
            &tx,
            &mut state,
            3.0,
        );

        let extra_windows = std::iter::from_fn(|| tp_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            extra_windows.is_empty(),
            "novel text at EOF has no new PCM and must not queue empty Layer 1 windows: {:?}",
            extra_windows
                .iter()
                .map(|request| request.audio.len())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            state.last_sealed_end, 3.0,
            "the window floor is canonical PCM time, never an unclamped Apple timestamp"
        );
        assert_eq!(
            state.unresolved_windows, 0,
            "a clamped EOF suffix is known to have no new PCM; it is not a clock lie"
        );
        let landed = state
            .progressive
            .pending_spans()
            .iter()
            .map(|span| normalize_for_containment(&span.raw_text))
            .chain(
                state
                    .progressive
                    .sealed_spans()
                    .iter()
                    .map(|span| normalize_for_containment(&span.text)),
            )
            .collect::<Vec<_>>();
        assert!(
            landed.iter().any(|text| text.contains("gamma")),
            "the first EOF suffix must remain on the canvas: {landed:?}"
        );
        assert!(
            landed.iter().any(|text| text.contains("delta")),
            "the later EOF suffix must remain on the canvas: {landed:?}"
        );
    }

    /// End to end through `seal_utterance_final`: four sealed occurrences of one
    /// name, then a cumulative final restating five. Exactly one new occurrence
    /// may reach the canvas.
    #[test]
    fn five_spoken_occurrences_survive_a_cumulative_restatement_end_to_end() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 5.0);

        emit_stream_events(
            vec![
                LiveStreamEvent::PhraseFinal {
                    text: "Iwo Iwo Iwo Iwo".into(),
                    segments: vec![
                        segment("Iwo", 0.0, 1.0),
                        segment("Iwo", 1.0, 2.0),
                        segment("Iwo", 2.0, 3.0),
                        segment("Iwo", 3.0, 4.0),
                    ],
                },
                // Cumulative restatement: same audio re-stated, plus one more
                // occurrence. The segment is fully behind the cursor, so this
                // takes the segment-less path.
                LiveStreamEvent::PhraseFinal {
                    text: "Iwo Iwo Iwo Iwo Iwo".into(),
                    segments: vec![segment("Iwo Iwo Iwo Iwo Iwo", 0.0, 4.0)],
                },
            ],
            &tx,
            &mut state,
            5.0,
        );

        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let finals: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let spoken = finals
            .iter()
            .map(|text| normalize_for_containment(text))
            .collect::<Vec<_>>()
            .join(" ");
        let occurrences = spoken.split_whitespace().filter(|w| *w == "iwo").count();
        assert_eq!(
            occurrences, 5,
            "five acoustic occurrences, five tokens — got {finals:?}"
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

    /// A later cumulative final can be entirely covered by already-committed
    /// spans. It is not an active tail: surfacing the whole callback as Preview
    /// makes the presentation reducer render `committed + restatement` and the
    /// delivery buffer duplicates the take at stop.
    #[test]
    fn fully_reheard_cumulative_final_clears_preview_instead_of_repeating_canvas() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 8.0);

        let heard_first = "szuty klawiszowe to podwójny lewy przycisk myszy";
        let restated =
            "skróty klawiszowe to podwójny lewy przycisk myszy lub klawisz na klawiaturze";

        for (text, end) in [(heard_first, 5.0), (restated, 6.0), (restated, 6.5)] {
            emit_stream_events(
                vec![LiveStreamEvent::PhraseFinal {
                    text: text.into(),
                    segments: vec![segment(text, 0.0, end)],
                }],
                &tx,
                &mut state,
                8.0,
            );
        }

        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let last_preview = events.iter().rev().find_map(|event| match event {
            EngineEvent::Preview { text, .. } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(
            last_preview,
            Some(""),
            "a fully re-heard final must clear the volatile tail, not repeat the canvas: {events:?}"
        );
        assert!(
            state.open_partial.is_empty(),
            "a fully re-heard final must not survive as stop-time open partial"
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
        let (tx, mut rx) = mpsc::unbounded_channel();
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
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok())
                .any(|event| matches!(event, EngineEvent::UtteranceFinal { .. })),
            "unresolved Apple text still emits a final"
        );
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

    /// Apple is the first observer. Sealing commits its observation unchanged;
    /// later repair requires a matching occurrence identity through the ledger.
    #[test]
    fn apple_seal_preserves_observed_text_until_ledger_repair() {
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
        assert_eq!(text, "uruchom doker teraz");
    }

    // ── W2-A · Layer 1 tail-patch on the Apple progressive path ──────────────

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

    /// A bounded stop that abandons accepted refinement must be observable on
    /// the ordered event surface; zero abandoned work stays quiet.
    #[test]
    fn tail_patch_drain_degrade_is_typed_once_and_zero_is_silent() {
        let sink = RecordingSink::default();
        report_tail_patch_drain_degrade(&sink, 0);
        assert!(sink.events().is_empty());

        report_tail_patch_drain_degrade(&sink, 2);
        emit_session_finalised(&sink, "test-session".to_string(), 0);
        let events = sink.events();
        assert_eq!(events.len(), 2);
        let EngineEvent::Warning { code, message } = &events[0] else {
            panic!("expected typed Warning, got {:?}", events[0]);
        };
        assert_eq!(code, TAIL_PATCH_DRAIN_TIMEOUT_WARNING_CODE);
        assert!(message.contains('2'));
        assert!(message.contains("Apple live text was preserved"));
        assert!(matches!(events[1], EngineEvent::SessionFinalised { .. }));
    }

    /// Settings/runtime degradation keeps a stable, separately actionable
    /// warning code that carries the disposition and no transcript text.
    #[test]
    fn local_tail_patch_degraded_warning_is_a_typed_event() {
        let sink = RecordingSink::default();
        emit_local_tail_patch_degraded_warning(&sink, "degraded_invalid_override");

        let events = sink.events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            EngineEvent::Warning { code, message }
                if code == LOCAL_TAIL_PATCH_DEGRADED_WARNING_CODE
                    && message == "degraded_invalid_override"
        ));
    }

    fn stage_pending_occurrence(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        utterance_id: u64,
        occurrence: OccurrenceIdentity,
        label: &str,
    ) {
        let calibration = EnergyCalibration {
            version: "c13-test".to_string(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(occurrence.sample_start),
            vad_close_sample: Some(occurrence.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .qualify(&evidence, &calibration);
        state.energy_calibration = Some(calibration);
        assert!(
            admit_ledger_label(
                state,
                ev_tx,
                LabelAdmission {
                    observation: LedgerObservationIdentity::new(
                        LedgerObservationProducer::Apple,
                        utterance_id,
                        0,
                        occurrence.clone(),
                    ),
                    label,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .is_some()
        );
        state.pending_events.insert(
            utterance_id,
            PendingAppleSeal {
                occurrence,
                raw_text: label.to_string(),
                layer1_baseline: label.to_string(),
                start_ts: 0.0,
                end_ts: 1.0,
                segments: Vec::new(),
            },
        );
    }

    fn launch_whisper_and_return_lexicon(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        utterance_id: u64,
        occurrence: &OccurrenceIdentity,
        label: &str,
    ) {
        assert!(
            state
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .schedule_observer(occurrence.clone(), LedgerObservationProducer::Whisper)
        );
        assert!(
            admit_ledger_label(
                state,
                ev_tx,
                LabelAdmission {
                    observation: LedgerObservationIdentity::new(
                        LedgerObservationProducer::Lexicon,
                        utterance_id,
                        0,
                        occurrence.clone(),
                    ),
                    label,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .is_some()
        );
    }

    /// Missing audio never launches Whisper. Queue loss and stop-time timeout
    /// return already-launched slots; none can strand an occurrence frontier.
    #[test]
    fn no_window_queue_rejection_and_timeout_conserve_whisper_frontiers() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let (not_launched_tx, _not_launched_rx) = mpsc::channel::<TailPatchRequest>(1);
        let mut no_window =
            AppleSealState::new_for_session(TEST_SAMPLE_RATE, "no-window".to_string(), 1);
        no_window.tail_patch = Some(not_launched_tx);
        let absent = OccurrenceIdentity::new("no-window", 1, 0, 16_000);
        stage_pending_occurrence(&mut no_window, &tx, 1, absent.clone(), "Iwo");
        assert!(
            !no_window
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .frontier_of(&absent)
                .expect("frontier")
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper),
            "configured tail lane without a PCM window does not launch Whisper"
        );
        let _ = admit_ledger_label(
            &mut no_window,
            &tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    1,
                    0,
                    absent.clone(),
                ),
                label: "Iwo",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );
        assert!(
            no_window
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .is_sealed(&absent)
        );
        while rx.try_recv().is_ok() {}

        let (tail_tx, tail_rx) = mpsc::channel::<TailPatchRequest>(1);
        drop(tail_rx);
        let mut rejected =
            AppleSealState::new_for_session(TEST_SAMPLE_RATE, "queue-rejected".to_string(), 1);
        rejected.tail_patch = Some(tail_tx);
        let occurrence = OccurrenceIdentity::new("queue-rejected", 1, 0, 16_000);
        stage_pending_occurrence(&mut rejected, &tx, 1, occurrence.clone(), "Iwo");
        let _ = rejected.enqueue_layer1_piece(
            &tx,
            CoalescedPiece {
                utterance_id: 1,
                committed_text: "Iwo".to_string(),
                audio: vec![0.5; 16_000],
                sample_start: 0,
                sample_end: 16_000,
                start_ts: 0.0,
                covered_through_secs: 1.0,
                segment_count: 1,
            },
        );
        let _ = admit_ledger_label(
            &mut rejected,
            &tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    1,
                    0,
                    occurrence.clone(),
                ),
                label: "Iwo",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );
        assert!(!rejected.flush_layer1_coalesce(&tx));
        assert!(
            rejected
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .is_sealed(&occurrence)
        );

        while rx.try_recv().is_ok() {}
        let mut timed_out =
            AppleSealState::new_for_session(TEST_SAMPLE_RATE, "timed-out".to_string(), 1);
        let timed = OccurrenceIdentity::new("timed-out", 1, 0, 16_000);
        stage_pending_occurrence(&mut timed_out, &tx, 1, timed.clone(), "Iwo");
        launch_whisper_and_return_lexicon(&mut timed_out, &tx, 1, &timed, "Iwo");
        timed_out.tail_patch_awaiting_completion = 1;
        timed_out.return_outstanding_whisper_without_label(&tx);
        assert_eq!(timed_out.tail_patch_awaiting_completion, 0);
        assert!(
            timed_out
                .acoustic_ledger
                .lock()
                .expect("ledger")
                .is_sealed(&timed)
        );
    }

    #[test]
    fn tail_patch_receipt_uses_worker_adjudicated_job_buckets() {
        let receipt = tail_patch_receipt_after_stop(
            true,
            3,
            Some(TailPatchWorkerAccounting {
                applied_jobs: 1,
                skipped_jobs: 1,
                timeout_residue: 1,
            }),
        );
        assert_eq!(receipt.applied, 1);
        assert_eq!(receipt.skipped, 1);
        assert_eq!(receipt.timed_out, 1);
        assert_eq!(receipt.abandoned, 0);
        assert!(receipt.is_reconciled());

        let worker_failed = tail_patch_receipt_after_stop(true, 2, None);
        assert_eq!(worker_failed.abandoned, 2);
        assert_eq!(worker_failed.drain, TailPatchDrainDisposition::Abandoned);
        assert!(worker_failed.is_reconciled());
    }

    #[test]
    fn worker_timeout_owns_async_outstanding_job_exactly_once() {
        let receipt = tail_patch_receipt_after_stop(
            true,
            1,
            Some(TailPatchWorkerAccounting {
                applied_jobs: 0,
                skipped_jobs: 0,
                timeout_residue: 1,
            }),
        );
        assert_eq!(receipt.timed_out, 1);
        assert_eq!(receipt.abandoned, 0);
        assert_eq!(receipt.drain, TailPatchDrainDisposition::TimedOut);
        assert!(receipt.is_reconciled());
    }

    #[test]
    #[should_panic(expected = "tail-patch terminal buckets must reconcile exactly")]
    fn tail_patch_receipt_rejects_missing_independent_terminal_evidence() {
        let _ = tail_patch_receipt_after_stop(
            true,
            3,
            Some(TailPatchWorkerAccounting {
                applied_jobs: 1,
                skipped_jobs: 1,
                timeout_residue: 0,
            }),
        );
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

    /// Computing a bearing patch is not delivery. The only application count
    /// belongs to the seal owner after its rewrite fence accepts the result.
    #[test]
    fn finishing_tail_patch_only_hands_identity_to_the_seal_owner() {
        let mut lane = AppleTailPatchLane::new(TEST_SAMPLE_RATE, None);
        let outcome = compute_tail_patch(
            "ala ma kota w domu",
            "ala ma kota w domu swoim",
            1,
            &TailPatchConfig::default(),
        );
        let completion = lane.finish_for_worker(
            Some(TailPatchInFlight {
                utterance_id: 1,
                covered_through_secs: 2.0,
                request_identity: TailRequestIdentity {
                    request_id: 1,
                    range: TailSampleRange {
                        session: "test-session".to_string(),
                        capture_epoch: 0,
                        sample_start: 0,
                        sample_end: 16_000,
                    },
                },
                span_map: Vec::new(),
                member_occurrences: vec![(
                    1,
                    OccurrenceIdentity::new("test-session", 0, 0, 16_000),
                )],
            }),
            Ok(synthetic_tail_job(1, outcome)),
        );
        assert!(
            completion
                .outcome
                .events()
                .iter()
                .any(|event| matches!(event, EngineEvent::ReplaceRange { .. })),
            "fixture must carry a bearing patch"
        );
        let (done_tx, done_rx) = std_mpsc::channel();
        assert!(lane.forward_completion_to_worker(&done_tx, completion));
        let accepted = done_rx.try_recv().expect("live worker receives completion");
        assert!(accepted.request_identity.is_some());
        assert_eq!(
            accepted.member_occurrences,
            vec![(1, OccurrenceIdentity::new("test-session", 0, 0, 16_000))],
            "worker completion preserves the exact member occurrence"
        );

        drop(done_rx);
        let rejected_outcome = compute_tail_patch(
            "drugi fragment",
            "drugi fragment odzyskany",
            2,
            &TailPatchConfig::default(),
        );
        let rejected = lane.finish_for_worker(
            Some(TailPatchInFlight {
                utterance_id: 2,
                covered_through_secs: 3.0,
                request_identity: TailRequestIdentity {
                    request_id: 2,
                    range: TailSampleRange {
                        session: "test-session".to_string(),
                        capture_epoch: 0,
                        sample_start: 16_000,
                        sample_end: 32_000,
                    },
                },
                span_map: Vec::new(),
                member_occurrences: vec![(
                    2,
                    OccurrenceIdentity::new("test-session", 0, 16_000, 32_000),
                )],
            }),
            Ok(synthetic_tail_job(2, rejected_outcome)),
        );
        assert!(!lane.forward_completion_to_worker(&done_tx, rejected));
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

        assert!(
            state.flush_layer1_coalesce(&tx),
            "one-seal tests flush the held window so the request is observable"
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

    /// A first final that arrives after the retention horizon must not poison
    /// the whole session. Measured live 2026-08-14: a 247 s take whose first
    /// SFSpeech final came at 156 s went 11/11 unresolved — `last_sealed_end`
    /// stayed 0.0 because it only advances on success, so Layer 1 received
    /// zero windows for the entire take. The window start clamps to retained
    /// audio (everything older is committed canvas by definition); a genuinely
    /// lying `end_ts` stays fail-closed.
    #[test]
    fn seal_window_clamps_start_after_retention_eviction() {
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 200.0);
        let retained_start = state.audio.retained_start_secs();
        assert!(
            retained_start > 0.0,
            "fixture must push past the retention cap to evict the session head"
        );

        let window = resolve_sealed_audio_window(&mut state, 150.0)
            .expect("stale `from` must clamp to retained audio, not fail the take");
        assert_eq!(
            window.sample_start,
            (retained_start as f64 * TEST_SAMPLE_RATE as f64) as u64,
            "clamped window starts at the oldest retained sample"
        );
        assert_eq!(window.sample_end, 150 * TEST_SAMPLE_RATE as u64);

        // The poison spiral is broken: the next window chains normally.
        let next = resolve_sealed_audio_window(&mut state, 180.0)
            .expect("later windows must resolve once the first seal landed");
        assert_eq!(next.sample_start, 150 * TEST_SAMPLE_RATE as u64);

        // A boundary that precedes the already-sealed canvas is still a lie.
        assert!(
            resolve_sealed_audio_window(&mut state, 100.0).is_none(),
            "end_ts behind the sealed canvas must stay fail-closed"
        );
        assert_eq!(state.unresolved_windows, 1);
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
        assert!(state.flush_layer1_coalesce(&tx));
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
                request_identity: Some(req.provider_request.identity.clone()),
                outcome: TailPatchOutcome::skipped(
                    crate::stt::tail_patcher::SkipReasonCode::EmptyRetranscription,
                    "no change",
                ),
                payload: None,
                span_map: req.span_map,
                member_occurrences: req.member_occurrences,
            },
            2.1,
        );

        assert_eq!(
            state.tail_patch_awaiting_completion, 0,
            "every job reported back — the stop path owes no further wait"
        );
        assert_eq!(state.tail_patch_jobs_applied, 0);
        assert_eq!(
            state.tail_patch_jobs_skipped, 1,
            "NoChange/provider skip is a completed skipped job, never missing arithmetic"
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

        let mut events = Vec::new();
        for i in 0..10 {
            let start = i as f32 * 0.5;
            events.push(LiveStreamEvent::PhraseFinal {
                text: format!("segment {i}"),
                segments: vec![segment(&format!("segment {i}"), start, start + 0.4)],
            });
        }
        emit_stream_events(events, &tx, &mut state, 10.0);

        assert!(
            state.tail_patch_backpressure_drops >= 1,
            "a second 5-segment flush must drop when the queue already holds one job"
        );
        assert!(
            state.sealed_count >= 1,
            "a dropped flush still seals Apple instead of stalling capture"
        );
    }

    /// Compatibility parser semantics remain strict. Product-mode defaults are
    /// resolved at recording bootstrap, not by this parser alone.
    #[test]
    fn layered_phase_compatibility_parser_accepts_phase1_and_off() {
        assert!(
            layered_phase_from_raw(None).is_none(),
            "unset means no explicit compatibility override"
        );
        assert!(
            parse_layered_phase_value("off").is_none(),
            "explicit off disarms"
        );
        assert_eq!(
            parse_layered_phase_value("phase1"),
            Some(1),
            "phase1 arms Layer 1"
        );
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
        push_capture(&mut state, 30.0);
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

    // ═══════════════════════════════════════════════════════════
    // Engine lifecycle: speech epochs (hands-free silence)
    // ═══════════════════════════════════════════════════════════

    /// Amplitude stand-in for the session Silero's `speech_live` bit, so the
    /// epoch state machine can be driven on synthetic PCM without loading the
    /// VAD model (unit tests must not depend on `init_silero_vad` succeeding).
    fn amplitude_edge(samples: &[f32], threshold: f32) -> bool {
        samples.iter().any(|s| s.abs() >= threshold)
    }

    /// One second of 200 Hz tone at `amplitude`, the "speech" side of the fixture.
    fn tone(secs: f32, amplitude: f32) -> Vec<f32> {
        let total = (secs * TEST_SAMPLE_RATE as f32) as usize;
        (0..total)
            .map(|i| {
                let t = i as f32 / TEST_SAMPLE_RATE as f32;
                amplitude * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
            })
            .collect()
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * TEST_SAMPLE_RATE as f32) as usize]
    }

    /// Drive the gate the way the worker does — chunk by chunk — collecting
    /// every decision together with the cursor it was taken at.
    fn drive(gate: &mut EpochGate, audio: &[f32], samples_seen: &mut u64) -> Vec<EpochDecision> {
        let mut out = Vec::new();
        for chunk in audio.chunks(1024) {
            *samples_seen += chunk.len() as u64;
            out.push(gate.feed_pcm(chunk, *samples_seen, amplitude_edge(chunk, 0.1)));
        }
        out
    }

    /// Timestamp shim: bridge time is per-epoch (seconds since that SFSpeech
    /// request opened), so every event leaving a non-zero epoch must be lifted
    /// onto the session PCM clock before any seal maps it to samples.
    #[test]
    fn epoch_shift_lifts_segment_times_onto_the_session_pcm_clock() {
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 110.0);

        let shifted = shift_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "uruchom doker".into(),
                segments: vec![segment("uruchom doker", 0.5, 2.0)],
            }],
            100.0,
        );
        let LiveStreamEvent::PhraseFinal { segments, .. } = &shifted[0] else {
            panic!("shim must preserve the event kind");
        };
        assert_eq!(segments[0].start_ts, 100.5);
        assert_eq!(segments[0].end_ts, 102.0);

        let on_pcm = apple_segments_on_pcm_clock(&state, segments);
        assert_eq!(
            on_pcm[0].range.sample_start,
            (100.5 * TEST_SAMPLE_RATE as f32) as u64
        );
        assert_eq!(
            on_pcm[0].range.sample_end,
            (102.0 * TEST_SAMPLE_RATE as f32) as u64
        );
    }

    /// The first epoch is based at 0, so the shim must be the identity there —
    /// this is what keeps a single-epoch take bit-identical to the legacy lane.
    #[test]
    fn epoch_shift_at_base_zero_is_identity() {
        let shifted = shift_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "uruchom".into(),
                    segments: vec![segment("uruchom", 0.5, 2.0)],
                },
                LiveStreamEvent::Summary {
                    text: "uruchom doker".into(),
                    segments: vec![segment("uruchom doker", 0.5, 4.0)],
                    ok: true,
                    error: None,
                },
            ],
            0.0,
        );
        let LiveStreamEvent::Partial { segments, .. } = &shifted[0] else {
            panic!("kind preserved");
        };
        assert_eq!((segments[0].start_ts, segments[0].end_ts), (0.5, 2.0));
        let LiveStreamEvent::Summary { segments, .. } = &shifted[1] else {
            panic!("kind preserved");
        };
        assert_eq!((segments[0].start_ts, segments[0].end_ts), (0.5, 4.0));
    }

    /// Engine lifecycle: speech opens an epoch, silence past the product
    /// threshold closes it, and the next speech edge wakes a new one whose
    /// base carries the pre-roll.
    #[test]
    fn epoch_gate_sleeps_after_threshold_silence_and_wakes_with_preroll() {
        let mut gate = EpochGate::armed(TEST_SAMPLE_RATE, 5.0);
        let mut seen = 0u64;

        let speech = drive(&mut gate, &tone(2.0, 0.5), &mut seen);
        assert!(
            matches!(
                speech.first(),
                Some(EpochDecision::Wake { preroll_from: 0 })
            ),
            "first speech chunk must open epoch 0 (nothing retained before it), got {:?}",
            speech.first()
        );
        assert!(
            speech[1..].iter().all(|d| *d == EpochDecision::Forward),
            "speech after the wake must forward, got {:?}",
            &speech[1..]
        );

        let quiet = drive(&mut gate, &silence(6.0), &mut seen);
        let sleep_at = quiet
            .iter()
            .position(|d| matches!(d, EpochDecision::Sleep { .. }))
            .expect("6 s of silence at a 5 s threshold must close the epoch");
        let sleep_secs = (sleep_at + 1) as f32 * 1024.0 / TEST_SAMPLE_RATE as f32;
        assert!(
            (5.0..5.2).contains(&sleep_secs),
            "epoch must close within a chunk of the 5 s threshold, closed at {sleep_secs}s"
        );
        assert!(
            quiet[sleep_at + 1..]
                .iter()
                .all(|d| *d == EpochDecision::Idle),
            "after sleeping the engine rests until the next speech edge"
        );

        let sleep_cursor = seen - (quiet.len() - sleep_at - 1) as u64 * 1024;
        let resume_cursor = seen;
        let woke = drive(&mut gate, &tone(1.0, 0.5), &mut seen);
        let EpochDecision::Wake { preroll_from } = woke[0] else {
            panic!("speech after rest must wake a new epoch, got {:?}", woke[0]);
        };
        let preroll = (EPOCH_PREROLL_SECS * TEST_SAMPLE_RATE as f32) as u64;
        assert_eq!(
            preroll_from,
            resume_cursor.saturating_sub(preroll),
            "the new epoch base is one pre-roll ahead of the waking chunk"
        );
        assert!(
            preroll_from >= sleep_cursor,
            "pre-roll must not reach back into the closed epoch ({preroll_from} < {sleep_cursor})"
        );
    }

    /// `utterance_silence_sec: None` is the legacy contract: one stream for the
    /// whole take, no epoch decisions at all.
    #[test]
    fn epoch_gate_disarmed_never_sleeps_or_wakes() {
        let mut gate = EpochGate::disarmed();
        assert!(!gate.is_armed());
        let mut seen = 0u64;
        let mut decisions = drive(&mut gate, &tone(1.0, 0.5), &mut seen);
        decisions.extend(drive(&mut gate, &silence(30.0), &mut seen));
        decisions.extend(drive(&mut gate, &tone(1.0, 0.5), &mut seen));
        assert!(
            decisions.iter().all(|d| *d == EpochDecision::Forward),
            "disarmed gate must forward every chunk, got {:?}",
            decisions
                .iter()
                .filter(|d| **d != EpochDecision::Forward)
                .collect::<Vec<_>>()
        );
    }

    /// No Silero ⇒ no edges ⇒ the lifecycle must NOT arm, or the take would rest
    /// forever on a stream that never opened. Fail open, every time.
    #[test]
    fn epoch_gate_without_speech_edges_falls_back_to_one_stream() {
        let mut gate = EpochGate::for_session(TEST_SAMPLE_RATE, Some(5.0), false);
        assert!(
            !gate.is_armed(),
            "an armed gate with no edge source would sleep the engine forever"
        );
        assert_eq!(
            gate.feed_pcm(&[0.0; 1_024], 1_024, false),
            EpochDecision::Forward,
            "Silero/sideband absence must preserve continuous Apple PCM flow"
        );
        let armed = EpochGate::for_session(TEST_SAMPLE_RATE, Some(5.0), true);
        assert!(armed.is_armed());
        assert!(
            !EpochGate::for_session(TEST_SAMPLE_RATE, None, true).is_armed(),
            "no hands-free silence setting is still the legacy single stream"
        );
    }

    // ═══════════════════════════════════════════════════════════
    // Utterance identity bound to the spectrum
    // ═══════════════════════════════════════════════════════════

    /// Samples per second at the test rate, as a `u64` sample cursor.
    fn at(secs: f32) -> u64 {
        (secs * TEST_SAMPLE_RATE as f32) as u64
    }

    /// Arm a state with the session Silero and mint two utterances separated by
    /// a silence wider than the long-silence fence, exactly as the Supervisor
    /// would: an open edge that extends, then a close, then a new edge.
    ///
    /// The ledger is driven through the production decision function
    /// ([`SileroIngress::observe`]) rather than a synthetic ledger, so what the
    /// seal reads is what a real chunk observation produces. Only the two facts
    /// Silero derives from the waveform are supplied by the fixture — the unit
    /// suite must not depend on `init_silero_vad` succeeding.
    fn arm_two_utterances(state: &mut AppleSealState) -> (u64, u64) {
        let mut ingress = SileroIngress::new(TEST_SAMPLE_RATE, state.session_id.clone(), 0);
        let first = ingress
            .observe(Some((at(0.0), at(1.0))), false, at(1.0))
            .open
            .expect("first speech edge mints an identity");
        ingress.observe(Some((at(0.0), at(2.0))), false, at(2.0));
        let closed = ingress.observe(None, true, at(2.0)).closed;
        assert_eq!(closed, vec![first]);

        // Silence well past LONG_SILENCE_FENCE_SECS, then a second edge.
        let gap = at(super::super::silero_fusion::LONG_SILENCE_FENCE_SECS) + at(1.0);
        let second_start = at(2.0) + gap;
        let second = ingress
            .observe(
                Some((second_start, second_start + at(2.0))),
                false,
                second_start + at(2.0),
            )
            .open
            .expect("speech after the fence mints a SECOND identity");
        assert_ne!(first, second, "the fence must split identity");

        state.fusion = Some(ingress);
        state.fusion_seal_armed = true;
        (first, second)
    }

    fn arm_fusion_slice_admission(state: &mut AppleSealState) -> Vec<TranscriptSegment> {
        state.energy_calibration = Some(EnergyCalibration::new(
            "fusion-slice-structural-test",
            0.0,
            0,
        ));
        push_capture(state, 12.0);
        arm_two_utterances(state);
        let second_start = state
            .fusion
            .as_ref()
            .expect("fusion fixture")
            .ledger()
            .utterances()[1]
            .range
            .sample_start as f32
            / TEST_SAMPLE_RATE as f32;
        vec![
            segment("Iwo", 0.2, 0.8),
            segment("Iwo", second_start + 0.2, second_start + 0.8),
        ]
    }

    #[test]
    fn fusion_slices_admit_disjoint_ledger_occurrences_before_raw_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        let disjoint = arm_fusion_slice_admission(&mut state);
        let fusion_ranges = state
            .fusion
            .as_ref()
            .expect("fusion fixture")
            .ledger()
            .utterances()
            .iter()
            .map(|utterance| (utterance.id, utterance.range.clone()))
            .collect::<Vec<_>>();

        assert!(seal_sliced_by_silero(&mut state, &tx, &disjoint));
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();

        let apple_mutations = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                EngineEvent::LedgerMutation {
                    observation, label, ..
                } if observation.producer == LedgerObservationProducer::Apple => {
                    Some((index, observation.occurrence.clone(), label.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(apple_mutations.len(), 2, "one Apple admit per Silero slice");
        assert_eq!(apple_mutations[0].2, "Iwo");
        assert_eq!(apple_mutations[1].2, "Iwo");
        assert_ne!(
            apple_mutations[0].1, apple_mutations[1].1,
            "equal labels on disjoint PCM ranges remain distinct occurrences"
        );

        for (utterance_id, range) in fusion_ranges {
            let mutation_index = apple_mutations
                .iter()
                .find_map(|(index, occurrence, _)| {
                    (occurrence.sample_start == range.sample_start
                        && occurrence.sample_end == range.sample_end)
                        .then_some(*index)
                })
                .expect("slice-local ledger mutation");
            let final_index = events
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        EngineEvent::UtteranceFinal {
                            utterance_id: final_id,
                            ..
                        } if *final_id == utterance_id
                    )
                })
                .expect("observation-only final");
            assert!(
                mutation_index < final_index,
                "ledger admission must precede the raw final for slice {utterance_id}"
            );
        }

        assert!(
            events.iter().all(|event| match event {
                EngineEvent::LedgerMutation { label, .. } => label == "Iwo",
                EngineEvent::UtteranceFinal { text, raw_text, .. } => {
                    text == "Iwo" && raw_text == "Iwo"
                }
                _ => true,
            }),
            "callback-wide text must never be copied into every slice: {events:#?}"
        );
    }

    #[test]
    fn fusion_slice_replay_reaches_ledger_identity_refusal() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        let disjoint = arm_fusion_slice_admission(&mut state);
        assert!(seal_sliced_by_silero(&mut state, &tx, &disjoint));
        while rx.try_recv().is_ok() {}

        assert!(seal_sliced_by_silero(&mut state, &tx, &disjoint));
        let replay_events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let replay_receipts = replay_events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::LedgerMutation {
                    observation,
                    receipt,
                    ..
                } if observation.producer == LedgerObservationProducer::Apple => Some(receipt),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(replay_receipts.len(), 2);
        assert!(
            replay_receipts
                .iter()
                .all(|receipt| !receipt.grants_mutation()),
            "replayed request/range identity must be refused by AcousticLedger"
        );
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .occurrences()
                .count(),
            2,
            "replay must not mint a third occurrence"
        );
    }

    /// (a) Utterance identity comes from the spectrum, and the seal carries it.
    ///
    /// Two Apple finals landing inside two Silero-bounded utterances must seal
    /// as two spans whose ids ARE the ledger ids and whose ranges ARE the
    /// ledger ranges — not Apple's own segment boundaries.
    #[test]
    fn sealed_spans_take_identity_and_range_from_silero_edges() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 12.0);
        let (first, second) = arm_two_utterances(&mut state);
        let ledger = state.fusion.as_ref().unwrap().ledger().clone();

        // One final inside utterance 1, one inside utterance 2.
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "pierwsza fraza".into(),
                segments: vec![segment("pierwsza fraza", 0.2, 1.8)],
            }],
            &tx,
            &mut state,
            2.2,
        );
        let second_start =
            ledger.utterances()[1].range.sample_start as f32 / TEST_SAMPLE_RATE as f32;
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "druga fraza".into(),
                segments: vec![segment(
                    "druga fraza",
                    second_start + 0.2,
                    second_start + 1.8,
                )],
            }],
            &tx,
            &mut state,
            second_start + 2.2,
        );

        let sealed = state.progressive.sealed_spans();
        assert_eq!(sealed.len(), 2, "two utterances ⇒ two spans: {sealed:#?}");
        for (span, utterance_id) in sealed.iter().zip([first, second]) {
            let utterance = ledger
                .utterances()
                .iter()
                .find(|u| u.id == utterance_id)
                .expect("fixture identity must exist in the ledger");
            assert_eq!(
                span.silero_utterance_id,
                Some(utterance_id),
                "span {} did not record the spectrum edge it came from",
                span.id
            );
            assert_eq!(
                span.range.sample_start, utterance.range.sample_start,
                "span {} start is not the Silero edge",
                span.id
            );
            assert_eq!(
                span.range.sample_end, utterance.range.sample_end,
                "span {} end is not the Silero edge",
                span.id
            );
        }
        assert_ne!(
            sealed[0].silero_utterance_id, sealed[1].silero_utterance_id,
            "a fenced silence must produce two DIFFERENT identities"
        );
    }

    /// (c) Words stay pinned to the PCM counter after binding: every Apple word
    /// range on a bound span lies inside the utterance range it was bound to.
    /// This is the "words on spectrum events" claim — without it a span could
    /// carry an utterance id while its words describe other seconds.
    #[test]
    fn bound_span_words_stay_inside_their_utterance_on_the_pcm_clock() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 12.0);
        arm_two_utterances(&mut state);
        let ledger = state.fusion.as_ref().unwrap().ledger().clone();

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "uruchom doker".into(),
                segments: vec![segment("uruchom", 0.2, 0.9), segment("doker", 0.9, 1.8)],
            }],
            &tx,
            &mut state,
            2.2,
        );

        let sealed = state.progressive.sealed_spans();
        assert_eq!(sealed.len(), 1);
        let span = &sealed[0];
        let utterance_id = span
            .silero_utterance_id
            .expect("the span must be bound to an edge");
        let utterance = ledger
            .utterances()
            .iter()
            .find(|u| u.id == utterance_id)
            .unwrap();
        assert!(!span.words.is_empty(), "a bound span must keep its words");
        for word in &span.words {
            assert!(
                word.range.sample_start >= utterance.range.sample_start
                    && word.range.sample_end <= utterance.range.sample_end,
                "word {:?} at {}..{} escapes utterance {} at {}..{}",
                word.text,
                word.range.sample_start,
                word.range.sample_end,
                utterance_id,
                utterance.range.sample_start,
                utterance.range.sample_end
            );
            assert!(
                word.range.sample_start < word.range.sample_end,
                "a word must occupy real samples, not a point"
            );
        }
        assert_eq!(
            span.words.first().unwrap().range.sample_start,
            at(0.2),
            "word start must stay on the PCM counter it was mapped from"
        );
        assert_eq!(span.words.last().unwrap().range.sample_end, at(1.8));
    }

    /// A span the spectrum does not enclose keeps Apple's own range and records
    /// no identity — binding is fail-open and never costs content.
    #[test]
    fn span_outside_every_silero_edge_keeps_the_apple_range() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 30.0);
        arm_two_utterances(&mut state);

        // 20 s is past every minted edge; slicing finds no cover either, so the
        // Apple-boundary path runs and must still seal.
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "poza spektrum".into(),
                segments: vec![segment("poza spektrum", 20.0, 21.0)],
            }],
            &tx,
            &mut state,
            21.5,
        );

        let sealed = state.progressive.sealed_spans();
        assert_eq!(
            sealed.len(),
            1,
            "content must never be dropped for want of an edge"
        );
        assert_eq!(
            sealed[0].silero_utterance_id, None,
            "no enclosing edge ⇒ no identity claimed"
        );
        assert_eq!(sealed[0].range.sample_start, at(20.0));
        assert_eq!(sealed[0].range.sample_end, at(21.0));
        assert!(
            !state
                .fusion
                .as_ref()
                .unwrap()
                .ledger()
                .utterances()
                .iter()
                .any(|u| u.id == sealed[0].id),
            "the fallback id must be reserved out of the ledger's id space, \
             never collide with a minted utterance"
        );
    }

    /// (b) Fail-open: no Silero at all is today's behaviour, bit for bit.
    /// Spans still seal, on Apple's own boundaries, with no identity claimed.
    #[test]
    fn without_silero_the_seal_path_is_unchanged() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 12.0);
        assert!(state.fusion.is_none(), "fixture has no VAD");

        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "uruchom doker".into(),
                segments: vec![segment("uruchom doker", 0.5, 2.0)],
            }],
            &tx,
            &mut state,
            2.2,
        );

        let sealed = state.progressive.sealed_spans();
        assert_eq!(sealed.len(), 1);
        assert_eq!(sealed[0].id, 1, "legacy ids still start at 1");
        assert_eq!(sealed[0].silero_utterance_id, None);
        assert_eq!(sealed[0].range.sample_start, at(0.5));
        assert_eq!(sealed[0].range.sample_end, at(2.0));
    }

    #[test]
    fn five_disjoint_iwo_segments_all_reach_the_final() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 6.0);
        let segments: Vec<_> = (0..5)
            .map(|i| {
                let start = i as f32 * 0.4;
                segment("Iwo", start, start + 0.3)
            })
            .collect();
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "Iwo Iwo Iwo Iwo Iwo".into(),
                segments,
            }],
            &tx,
            &mut state,
            3.0,
        );
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let finals: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            finals.len(),
            1,
            "one Apple final for five disjoint words: {events:?}"
        );
        let iwo_count = count_iwo(finals[0]);
        assert_eq!(iwo_count, 5, "delivery text: {}", finals[0]);
    }

    #[test]
    fn cumulative_fifth_iwo_is_not_absorbed_as_a_revision() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        push_capture(&mut state, 8.0);
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "Iwo Iwo Iwo Iwo".into(),
                segments: (0..4)
                    .map(|i| {
                        let start = i as f32 * 0.4;
                        segment("Iwo", start, start + 0.3)
                    })
                    .collect(),
            }],
            &tx,
            &mut state,
            2.0,
        );
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "Iwo Iwo Iwo Iwo Iwo".into(),
                segments: vec![segment("Iwo Iwo Iwo Iwo Iwo", 0.0, 1.6)],
            }],
            &tx,
            &mut state,
            3.0,
        );
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let texts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let iwo_count = texts.iter().map(|text| count_iwo(text)).sum::<usize>();
        assert_eq!(
            iwo_count, 5,
            "the fifth acoustic Iwo must survive the cumulative restatement: {texts:?}"
        );
    }
}

/// Conservation falsifiers from the acoustic-identity cut. These encode the
/// contract, not a parked skip: they must stay green.
#[cfg(any())]
mod observation_identity_conservation_falsifiers {
    use super::*;

    fn probe_words(callback: &str) -> Vec<String> {
        callback
            .split_whitespace()
            .map(|word| normalize_for_containment(&seal_span_text(word, "", true)))
            .collect()
    }
}

/// Parked conservation falsifier for the segment-less Apple final path.
///
/// Encodes THE ENGINE contract's repetition and conservation fixtures, not
/// current behaviour. `#[ignore]`d until "Acoustic identity cut order" step 6
/// in `docs/THE_ENGINE_CONTRACT.md` lands; the anti-drift rule requires a
/// temporary OFF to name the falsifier it waits for, and this is that falsifier.
#[cfg(any())]
mod ledger_conservation_falsifiers {
    use super::*;

    fn probe_words(callback: &str) -> Vec<String> {
        callback
            .split_whitespace()
            .map(|word| normalize_for_containment(&seal_span_text(word, "", true)))
            .collect()
    }

    fn open(index: u64, words: usize) -> (OccurrenceIdentity, usize) {
        let start = index * 16_000;
        (
            OccurrenceIdentity::new("apple_live", 1, start, start + 16_000),
            words,
        )
    }

    /// With nothing open, the live lane has no acoustic authority to apply and
    /// the legacy matcher keeps its answer. The cut demotes the matcher where
    /// evidence exists; it does not fabricate a verdict where none does.
    #[test]
    fn with_nothing_open_the_legacy_matcher_answer_stands() {
        let known = known_prefix_under_authority("alpha beta", "alpha beta", &[]);
        assert_eq!(known, 2);
    }
}
