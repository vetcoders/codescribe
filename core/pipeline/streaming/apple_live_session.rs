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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesOrdered;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::agent::consultation::{
    ConsultationGroupAnswer, ConsultationInputQueue, ConsultationReadiness,
    PendingConsultationGroup, PreparedConsultationGroup, SealedConsultationInput,
};
use crate::asr_session::recorder::{
    LAYER1_DEGRADED_WARNING_CODE, Layer1DegradeReason, RecorderLayer1Lane,
    apply_recorder_lifecycle_event,
};
use crate::asr_session::{SessionId as Layer1SessionId, SessionInput as Layer1SessionInput};
use crate::audio::capture_receipt::{
    AcousticAvailability, AcousticSpeechEvidence, CaptureEnergyOwner, CaptureLevelAccumulator,
    CapturePathMeta, emit_capture_level_receipt,
};
use crate::audio::streaming_recorder::CaptureTurnIntent;
use crate::config::{FormattingPolicy, RuntimeSettingsSnapshot};
use crate::llm::ai_formatting::{
    AiFormatResult, AiFormatStatus, format_text_with_status_for_policy,
};
use crate::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
use crate::pipeline::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, MutationReceipt, NoAuthorityReason,
    ObservationIdentity as LedgerObservationIdentity,
    ObservationProducer as LedgerObservationProducer, OccurrenceIdentity, OverlapPinClass,
    RefuseReason, SealCoverageReceipt, SealCoverageStatus, SealRefusal,
};
use crate::pipeline::contracts::{
    EngineEvent, EventSink, PreviewPin, SessionConservationReceipt, SpeechIntegrity,
    SpeechIntegrityPhase, TranscriptSegment,
};
use crate::stt::apple_stt::{LiveStreamEvent, LiveStreamSession};
use crate::stt::tail_patcher::{SkipReasonCode, TailPatchConfig, TailPatchOutcome};
use crate::stt::tail_provider::{
    InProcessTailProvider, TailProviderPayload, TailProviderRequest, TailRequestIdentity,
    TailSampleRange, TimedTailSegment,
};

use super::layer1_window::{CoalesceFlush, CoalescedPiece, Layer1Coalesce};
use super::live_audio_buffer::{DEFAULT_RETENTION_SECS, LiveAudioBuffer, ResolvedAudioWindow};
use super::session::{
    LocalExecutionOwner, SessionConfig, TailPatchDrainDisposition, TailPatchJobInput,
    TailPatchJobResult, TailPatchSessionReceipt, compute_tail_patch_job, emit_session_finalised,
    log_tail_patch_session_receipt,
};
use super::silero_fusion::{
    ContextBounds, FusionContextMode, FusionWord, SileroIngress, bound_context_range,
    slice_apple_words,
};
use super::speech_progress::SpeechProgress;
use super::stream_log::append_to_stream_log;

/// How many sealed utterances may wait for Layer 1 before the seal path starts
/// dropping requests.
///
/// The seal path runs on the worker thread that also forwards PCM into the
/// SFSpeech bridge, so it must never block on this queue — a stalled worker is
/// a stalled capture. Bounded retries preserve exact requests under temporary
/// pressure; capacity exhaustion is an explicit per-occurrence failure.
const TAIL_PATCH_QUEUE_CAP: usize = 8;

/// Bounded transport ownership for occurrence formatter jobs. Saturation skips
/// Formatter scheduling for that occurrence; it never backpressures PCM.
const FORMATTER_QUEUE_CAP: usize = 8;

const CONSULTATION_QUEUE_CAP: usize = 16;

enum LiveConsultationRequest {
    Assess(SealedConsultationInput),
    Prepare(SealedConsultationInput),
    Finish(PendingConsultationGroup),
}

enum LiveConsultationResult {
    Assessed(Result<ConsultationReadiness>),
    Prepared(Result<PreparedConsultationGroup>),
    Answer(Result<ConsultationGroupAnswer>),
}

enum LiveConsultationReturn {
    Assessed(Result<ConsultationReadiness>),
    Prepared(Result<PreparedConsultationGroup>),
    Published(Result<()>),
}

/// Capture-thread grouping and admission. Provider futures live on the async
/// session; only this owner reads speech edges and advances the accepted prefix.
struct LiveConsultationCapture {
    queue: ConsultationInputQueue,
    requests: mpsc::Sender<LiveConsultationRequest>,
    returns: std_mpsc::Receiver<LiveConsultationReturn>,
    last_assessed: Option<SealedConsultationInput>,
    assessment_pending: bool,
    answers_pending: usize,
    speech_open: bool,
    refused: bool,
}

impl LiveConsultationCapture {
    fn observe(
        &mut self,
        ingest: &super::silero_fusion::SileroIngest,
        samples_seen: u64,
    ) -> Result<()> {
        self.speech_open = ingest.open.is_some();
        if ingest.speech_live {
            self.queue.resume_speech();
        }
        // A closing block is speech_live too. Only the actual close with no
        // successor open can nominate a candidate; silence alone cannot.
        if !ingest.closed.is_empty() && !self.speech_open {
            self.queue.append_boundary(samples_seen)?;
        }
        Ok(())
    }

    fn report_refusal(&mut self, events: &mpsc::UnboundedSender<EngineEvent>) {
        if !self.refused {
            let _ = events.send(EngineEvent::Warning {
                code: "max_consultation_refused".into(),
                message: "Max consultation could not settle; source audio and transcript remain authoritative. Accepted tools must not be replayed.".into(),
            });
        }
        self.refused = true;
    }

    fn tick(&mut self, state: &AppleSealState, events: &mpsc::UnboundedSender<EngineEvent>) {
        loop {
            let result = match self.returns.try_recv() {
                Ok(result) => result,
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    self.report_refusal(events);
                    // The session task vanished. This cannot certify tool
                    // cancellation; the retained executor still owns recovery.
                    self.assessment_pending = false;
                    self.answers_pending = 0;
                    break;
                }
            };
            match result {
                LiveConsultationReturn::Published(result) => {
                    self.answers_pending = self.answers_pending.saturating_sub(1);
                    if result.is_err() {
                        self.report_refusal(events);
                    }
                }
                LiveConsultationReturn::Assessed(result) => {
                    self.assessment_pending = false;
                    let Ok(assessment) = result else {
                        let _ = events.send(EngineEvent::Warning {
                            code: "max_assessment_unavailable".into(),
                            message: "Max could not assess this candidate. It remains pending; new speech can form a new candidate.".into(),
                        });
                        continue;
                    };
                    if self.refused || self.speech_open {
                        continue;
                    }
                    let ConsultationReadiness::Complete(input) = assessment else {
                        continue;
                    };
                    match self
                        .requests
                        .try_send(LiveConsultationRequest::Prepare(input))
                    {
                        Ok(()) => self.assessment_pending = true,
                        Err(_) => self.report_refusal(events),
                    }
                }
                LiveConsultationReturn::Prepared(result) => {
                    self.assessment_pending = false;
                    let prepared = match result {
                        Ok(prepared) => prepared,
                        Err(_) => {
                            self.report_refusal(events);
                            continue;
                        }
                    };
                    // A dropped preparation cannot execute. Revalidate AFTER
                    // disk I/O, while capture owns current speech and ledger.
                    if self.refused || self.speech_open {
                        continue;
                    }
                    let Some(fusion) = state.fusion.as_ref() else {
                        continue;
                    };
                    let speech = fusion.acoustic_speech_evidence();
                    // Reserve result transport BEFORE accepting effects. A
                    // successfully accepted handle cannot disappear on pressure.
                    let sender = self.requests.clone();
                    let Ok(permit) = sender.try_reserve() else {
                        self.report_refusal(events);
                        continue;
                    };
                    let ledger = state
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match self.queue.ready(&ledger, &speech) {
                        Ok(Some(current)) if &current == prepared.input() => {}
                        _ => continue,
                    }
                    // A stale assessment is expected after continuation. It
                    // never invokes the executor and never consumes the front.
                    let accepted = self.queue.authorize_prepared(prepared, &ledger, &speech);
                    match accepted {
                        Ok(pending) => {
                            let acknowledged = self.queue.acknowledge(&pending);
                            self.answers_pending += 1;
                            permit.send(LiveConsultationRequest::Finish(pending));
                            if acknowledged.is_err() {
                                self.report_refusal(events);
                            }
                        }
                        Err(_) => self.report_refusal(events),
                    }
                }
            }
        }
        if self.refused
            || self.speech_open
            || self.assessment_pending
            || self.answers_pending >= CONSULTATION_QUEUE_CAP
        {
            return;
        }
        let Some(fusion) = state.fusion.as_ref() else {
            return;
        };
        let speech = fusion.acoustic_speech_evidence();
        let ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while self.queue.skip_measured_silence(&ledger, &speech) {}
        let input = match self.queue.ready(&ledger, &speech) {
            Ok(Some(input)) => input,
            Ok(None) => return,
            Err(_) => {
                self.report_refusal(events);
                return;
            }
        };
        if self.last_assessed.as_ref() == Some(&input) {
            return;
        }
        match self
            .requests
            .try_send(LiveConsultationRequest::Assess(input.clone()))
        {
            Ok(()) => {
                self.last_assessed = Some(input);
                self.assessment_pending = true;
            }
            Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => self.report_refusal(events),
        }
    }

    fn settle(
        &mut self,
        state: &AppleSealState,
        events: &mpsc::UnboundedSender<EngineEvent>,
        end: u64,
    ) {
        self.speech_open = false;
        if self.queue.append_boundary(end).is_err() {
            self.report_refusal(events);
        }
        while self.queue.pending_groups() > 1 {
            if self.queue.join_front().is_err() {
                self.report_refusal(events);
                break;
            }
        }
        loop {
            self.tick(state, events);
            if !self.assessment_pending && self.answers_pending == 0 {
                break;
            }
            // The async session keeps draining providers, approvals and answer
            // publication. Microphone capture has already ended at this point.
            thread::sleep(LIVE_WORKER_QUANTUM);
        }
        if self.queue.pending_groups() > 0 {
            self.report_refusal(events);
        }
    }
}

fn deliver_consultation_result(
    result: LiveConsultationResult,
    returns: &std_mpsc::Sender<LiveConsultationReturn>,
    sink: &dyn EventSink,
) {
    let result = match result {
        LiveConsultationResult::Assessed(result) => LiveConsultationReturn::Assessed(result),
        LiveConsultationResult::Prepared(result) => LiveConsultationReturn::Prepared(result),
        LiveConsultationResult::Answer(result) => LiveConsultationReturn::Published(
            result.and_then(|answer| sink.on_consultation_completed(&answer)),
        ),
    };
    if returns.send(result).is_err() {
        warn!("Max capture owner closed before result acknowledgement; no execution replay");
    }
}

/// Total budget for the end-of-session closure loop across all Layer 1
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

/// Budget for stop-path text recovery after the live tail-patch drain.
///
/// The live drain is [`TAIL_PATCH_CLOSURE_TIMEOUT`] and `begin_drain` cannot
/// extend it. On take 9608b50e that drain completed (`timed_out=0`, 25 live
/// jobs already skipped) and the same 5s clock then ran the coverage requests:
/// three windows finished between 11:26:21.720Z and 11:26:24.611Z, and the
/// other three died at 11:26:25.327Z as `local execution cancelled or drain
/// deadline expired`. Five debt occurrences on that take cover about 77s of
/// PCM (3_685_376 utterance samples at 48 kHz). At the observed rate, roughly
/// 27s of PCM in 4.2s, five windows are about 12s of sequential work on a
/// model that is already warm. This cap is that phase only. It returns when
/// the jobs finish; a take with nothing to recover does not wait it out.
const SEAL_TEXT_RECOVERY_BUDGET: Duration = Duration::from_secs(20);

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

/// What the retired char-diff made of one Layer 1 job, and what actually
/// happened to that job's payload.
///
/// The two are separate facts and were being read as one. `verdict = "skipped"`
/// reads like a rejection, and three such lines on one archived take were read
/// as three lost Whisper labels; correlating PCM spans against Bus revisions
/// showed all three had been admitted and had persisted to a later published
/// revision. The verdict never decided that. The one-throne path admits
/// Whisper through `AcousticLedger` on occurrence identity, and `payload` is
/// forwarded on every successful job regardless of what the char-diff said.
///
/// So the receipt states both, and states which one has authority. Text-free by
/// construction: no transcript ever reaches a log line through here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LegacyCharDiffReceipt {
    /// The retired decision, as a token.
    verdict: &'static str,
    /// Whether the provider payload continued to occurrence admission. `true`
    /// for every `Ok` job — including `skipped` and `under_commit`.
    payload_forwarded: bool,
    /// Who actually decides admission. Never this verdict.
    admission_authority: &'static str,
}

/// Ledger admission is bound to occurrence identity, never to a text diff.
const LEDGER_ADMISSION_AUTHORITY: &str = "acoustic_ledger_occurrence_identity";

fn legacy_char_diff_receipt(
    outcome: &TailPatchOutcome,
    payload_forwarded: bool,
) -> LegacyCharDiffReceipt {
    LegacyCharDiffReceipt {
        verdict: match outcome {
            TailPatchOutcome::Patches(_) => "patches",
            TailPatchOutcome::NoChange => "no_change",
            TailPatchOutcome::UnderCommit(_) => "under_commit",
            TailPatchOutcome::Skipped { .. } => "skipped",
        },
        payload_forwarded,
        admission_authority: LEDGER_ADMISSION_AUTHORITY,
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
    /// Pins inside this half-open range may be admitted. Overlap context and
    /// the trailing second reserved for the next window stay outside it.
    admit_sample_start: u64,
    admit_sample_end: u64,
    /// Every exact occurrence whose launched Whisper slot this job must close.
    member_occurrences: Vec<(u64, OccurrenceIdentity)>,
}

// Bounds include unsent owned PCM; source retention remains independently bounded.
const LIVE_REFINEMENT_PENDING_CAP: usize = 8;
const LIVE_REFINEMENT_PCM_SECS: usize = 32;
const LIVE_WORKER_QUANTUM: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy)]
enum RefinementFailure {
    LaneGone,
    BacklogExhausted,
    PcmUnavailable,
    InvalidIdentity,
    NoLabel,
    StopDeadline,
    QualificationRefused,
}

impl RefinementFailure {
    fn code(self) -> &'static str {
        match self {
            Self::LaneGone => "live_refinement_lane_gone",
            Self::BacklogExhausted => "live_refinement_backlog_exhausted",
            Self::PcmUnavailable => "live_refinement_pcm_unavailable",
            Self::InvalidIdentity => "live_refinement_invalid_identity",
            Self::NoLabel => "live_refinement_no_label",
            Self::StopDeadline => "live_refinement_stop_deadline",
            Self::QualificationRefused => "live_refinement_qualification_refused",
        }
    }
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
    admit_sample_start: u64,
    admit_sample_end: u64,
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
    execution: Arc<LocalExecutionOwner>,
    jobs: FuturesOrdered<BoxFuture<'static, Result<TailPatchJobResult>>>,
    language: Option<String>,
    config: TailPatchConfig,
    provider: crate::stt::tail_provider::TailProviderId,
}

impl AppleTailPatchLane {
    /// Open an empty lane. `TailPatchConfig::from_env` is read once here so the
    /// whole session judges every patch against the same thresholds, even if the
    /// env flips mid-hold.
    fn new(
        _sample_rate: u32,
        language: Option<String>,
        provider: crate::stt::tail_provider::TailProviderId,
    ) -> Self {
        Self {
            execution: Arc::new(LocalExecutionOwner::default()),
            jobs: FuturesOrdered::new(),
            language,
            // F2: thresholds stay exactly where the shared primitive puts them.
            config: TailPatchConfig::from_env(),
            provider,
        }
    }

    /// Turn a sealed utterance into a Whisper gap-fill job and queue it. The job
    /// is only constructed — inference runs on a retained native worker, so
    /// this call never sits on the event-drain path.
    fn push_request(&mut self, mut req: TailPatchRequest) {
        req.provider_request.language = self.language.clone();
        let job = compute_tail_patch_job(
            &self.execution,
            TailPatchJobInput {
                utterance_id: req.utterance_id,
                committed_text: req.committed_text,
                neighbour_context: req.neighbour_context,
                audio: req.audio,
                request: req.provider_request,
                config: self.config,
            },
            self.provider,
        );
        self.push_job(job);
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
                // the verdict is reduced to a token before it reaches the log.
                // The payload is forwarded whatever the verdict says — the
                // receipt names both facts so neither can be read as the other.
                let receipt = legacy_char_diff_receipt(&job.outcome, true);
                debug!(
                    utterance_id = job.utterance_id,
                    legacy_verdict = receipt.verdict,
                    payload_forwarded = receipt.payload_forwarded,
                    admission_authority = receipt.admission_authority,
                    "Layer 1 char-diff verdict is diagnostic only; the payload continues to \
                     occurrence admission"
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
                    payload_forwarded = false,
                    admission_authority = LEDGER_ADMISSION_AUTHORITY,
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

/// Whether *this take* may open a paid formatter slot while it is still live.
///
/// The configuration predicate above answers "is a formatter reachable at all";
/// this one adds the only other question the worker has: does the gesture that
/// opened the microphone want per-occurrence formatting as it happens?
///
/// A one-turn composer take answers no. It is not deduplicated downstream and
/// it is not counted and discarded — the sender simply never exists, so
/// `schedule_formatter_after_terminal_label` cannot reserve a permit and no
/// `Formatter` observer is ever scheduled on the ledger frontier. The turn is
/// formatted once at terminal processing by the controller instead.
/// Max also refuses this occurrence lane: consultation admission reads sealed
/// groups, so scheduling its execution as a pre-seal observer would reverse
/// that dependency and could run tools on isolated words. Its retained Agent
/// capability belongs to the grouped consultation path, not this sender.
fn live_formatter_lane_is_armed(
    capture_turn: CaptureTurnIntent,
    ai_formatting_enabled: bool,
    policy: FormattingPolicy,
    lane_available: impl FnOnce() -> bool,
) -> bool {
    capture_turn.schedules_live_formatting()
        && !matches!(policy, FormattingPolicy::Max)
        && formatter_lane_is_armed(ai_formatting_enabled, policy, true)
        && lane_available()
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
    conservation: SessionConservationReceipt,
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
    .with_conservation(conservation)
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
        live_formatting_agent,
        acoustic_ledger,
        sample_rate,
        capture_device_name,
        language,
        stream_log_path,
        utterance_silence_sec,
        capture_turn,
        layer1,
        mut lifecycle_events,
        terminal_audio,
    } = config;
    // One owner for this capture epoch's acoustic evidence. This async arm is
    // the writer and the blocking Apple worker below is the reader; both hold
    // the same handle, so no process-global slot and no reset entrypoint sit
    // between them.
    let capture_energy = CaptureEnergyOwner::bind(session_id.clone(), capture_epoch);
    let mut capture_level = CaptureLevelAccumulator::bound_to(&capture_energy);
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
    let mut tail_patch_lane = AppleTailPatchLane::new(
        sample_rate,
        language.clone(),
        runtime_settings
            .tail_provider()
            .unwrap_or(crate::stt::tail_provider::TailProviderId::InProcess),
    );
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
    let formatter_on = live_formatter_lane_is_armed(
        capture_turn,
        runtime_settings.values().ai_formatting_enabled,
        runtime_settings.formatting_policy(),
        || {
            runtime_settings
                .llm_lanes()
                .formatting()
                .request_available()
        },
    );
    if !capture_turn.schedules_live_formatting() {
        info!(
            "One-turn capture: the live formatter lane stays unarmed for this take, so no \
             silence-delimited fragment can open a paid provider slot"
        );
    }
    let mut formatter_jobs = FuturesOrdered::<BoxFuture<'static, FormatterCompletion>>::new();
    let formatter_runtime_settings = Arc::clone(&runtime_settings);
    let formatter_language = language.clone();
    let (formatter_tx, mut formatter_rx) = mpsc::channel::<FormatterRequest>(FORMATTER_QUEUE_CAP);
    let (formatter_done_tx, formatter_done_rx) = std_mpsc::channel::<FormatterCompletion>();
    let worker_formatter_tx = formatter_on.then_some(formatter_tx);

    let (consultation_tx, mut consultation_rx) = mpsc::channel(CONSULTATION_QUEUE_CAP);
    let (consultation_return_tx, consultation_return_rx) = std_mpsc::channel();
    let mut consultation_assessments =
        FuturesOrdered::<BoxFuture<'static, Result<ConsultationReadiness>>>::new();
    let mut consultation_preparations =
        FuturesOrdered::<BoxFuture<'static, Result<PreparedConsultationGroup>>>::new();
    let mut consultation_answers =
        FuturesOrdered::<BoxFuture<'static, Result<ConsultationGroupAnswer>>>::new();
    if live_formatting_agent.is_some() && event_sink.consultation_destinations() != 1 {
        event_sink.on_event(&EngineEvent::Warning {
            code: "max_consultation_destination_unavailable".into(),
            message: "Live Max requires exactly one configured presentation destination.".into(),
        });
    }
    let live_formatting_agent = live_formatting_agent.filter(|_| {
        capture_turn.schedules_live_formatting()
            && runtime_settings.values().ai_formatting_enabled
            && runtime_settings.formatting_policy() == FormattingPolicy::Max
            && event_sink.consultation_destinations() == 1
    });
    let worker_consultation = live_formatting_agent
        .as_ref()
        .map(|_| LiveConsultationCapture {
            queue: ConsultationInputQueue::new(session_id.clone(), capture_epoch)
                .expect("recorder owns a valid capture identity"),
            requests: consultation_tx,
            returns: consultation_return_rx,
            last_assessed: None,
            assessment_pending: false,
            answers_pending: 0,
            speech_open: false,
            refused: false,
        });

    let worker_session_id = session_id.clone();
    let worker_capture_energy = capture_energy.clone();
    let worker_execution = Arc::clone(&tail_patch_lane.execution);
    let worker = thread::spawn(move || {
        apple_stream_worker(
            pcm_rx,
            ev_tx,
            worker_tp_tx,
            tp_done_rx,
            worker_formatter_tx,
            formatter_done_rx,
            AppleWorkerConfig {
                local_execution: worker_execution,
                sample_rate,
                capture_device_name,
                language: language.as_deref(),
                session_id: worker_session_id,
                capture_epoch,
                capture_energy: worker_capture_energy,
                runtime_settings,
                acoustic_ledger,
                settings_digest,
                utterance_silence_sec,
                terminal_audio,
                consultation: worker_consultation,
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
    let mut worker_finished = false;
    loop {
        tokio::select! {
            event = ev_rx.recv(), if !worker_finished => {
                match event {
                    // Same diagnostic artifact the VAD path writes: one line
                    // per committed utterance (CODESCRIBE_STREAM_LOG).
                    Some(event) => deliver_event(
                        &event,
                        event_sink.as_ref(),
                        stream_log_path.as_deref(),
                    ),
                    // Worker dropped the sender — stream finished.
                    None => worker_finished = true,
                }
            }
            Some(request) = consultation_rx.recv() => {
                match request {
                    LiveConsultationRequest::Assess(input) => {
                        if let Some(agent) = live_formatting_agent.as_ref() {
                            let agent = Arc::clone(agent);
                            let settings = Arc::clone(&formatter_runtime_settings);
                            consultation_assessments.push_back(Box::pin(async move {
                                agent.assess_group(input, &settings).await
                            }));
                        }
                    }
                    LiveConsultationRequest::Prepare(input) => {
                        if let Some(agent) = live_formatting_agent.as_ref() {
                            let agent = Arc::clone(agent);
                            let settings = Arc::clone(&formatter_runtime_settings);
                            consultation_preparations.push_back(Box::pin(async move {
                                tokio::task::spawn_blocking(move || agent.prepare_group(input, &settings))
                                    .await.map_err(|error| anyhow::anyhow!("Max preparation worker failed: {error}"))?
                            }));
                        }
                    }
                    LiveConsultationRequest::Finish(pending) => {
                        consultation_answers.push_back(Box::pin(pending.finish()));
                    }
                }
            }
            Some(result) = consultation_assessments.next() => {
                deliver_consultation_result(LiveConsultationResult::Assessed(result),
                    &consultation_return_tx, event_sink.as_ref());
            }
            Some(result) = consultation_preparations.next() => {
                deliver_consultation_result(LiveConsultationResult::Prepared(result),
                    &consultation_return_tx, event_sink.as_ref());
            }
            Some(result) = consultation_answers.next() => {
                // A fast provider must not overtake the source ledger events
                // already emitted by the capture worker on the other channel.
                while let Ok(event) = ev_rx.try_recv() {
                    deliver_event(&event, event_sink.as_ref(), stream_log_path.as_deref());
                }
                deliver_consultation_result(LiveConsultationResult::Answer(result),
                    &consultation_return_tx, event_sink.as_ref());
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
            // call itself runs on a retained native worker, so this loop
            // only ever schedules and collects — inference never sits on the
            // event-drain path (F1).
            Some(req) = tp_rx.recv(), if !tail_patch_in_flight => {
                info!(
                    session = %req.provider_request.identity.range.session,
                    capture_epoch = req.provider_request.identity.range.capture_epoch,
                    sample_start = req.provider_request.identity.range.sample_start,
                    sample_end = req.provider_request.identity.range.sample_end,
                    request_id = req.provider_request.identity.request_id,
                    disposition = "provider_started",
                    "live_refinement"
                );
                tail_patch_submitted = tail_patch_submitted.saturating_add(1);
                let inflight = TailPatchInFlight {
                    utterance_id: req.utterance_id,
                    request_identity: req.provider_request.identity.clone(),
                    admit_sample_start: req.admit_sample_start,
                    admit_sample_end: req.admit_sample_end,
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
                        None,
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
        if worker_finished
            && consultation_rx.is_closed()
            && consultation_rx.is_empty()
            && consultation_assessments.is_empty()
            && consultation_preparations.is_empty()
            && consultation_answers.is_empty()
        {
            break;
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
    // Their results cannot change the closed ledger. Cancel admission and join
    // actual executions before SessionFinalised; accounting is not execution.
    let mut outstanding_tail_patch_jobs = u64::from(tail_patch_in_flight);
    while tp_rx.try_recv().is_ok() {
        tail_patch_submitted = tail_patch_submitted.saturating_add(1);
        outstanding_tail_patch_jobs = outstanding_tail_patch_jobs.saturating_add(1);
    }
    if outstanding_tail_patch_jobs > 0 {
        warn!(
            outstanding_tail_patch_jobs,
            "Layer 1 results discarded after terminal accounting; joining retained execution"
        );
    }
    tail_patch_lane.execution.close_and_join().await;
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
    let mut conservation = SessionConservationReceipt::default();
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
            conservation = outcome.conservation.clone();
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
        conservation.clone(),
    );
    log_tail_patch_session_receipt(&receipt);
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
        conservation,
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
    speech_progress: SpeechProgress,
    last_integrity: Option<SpeechIntegrity>,
    utterance_id: u64,
    open_partial: String,
    open_partial_segments: Vec<TranscriptSegment>,
    sealed_count: u64,
    filtered_empty_drops: u64,
    /// Bounded PCM retention, so a sealed boundary can be resolved back to the
    /// audio behind it (Layer 1 tail-patch prerequisite).
    audio: LiveAudioBuffer,
    terminal_pcm: Option<super::live_audio_buffer::OwnedTerminalPcm>,
    /// Session time of the previous seal — the lower bound of the next
    /// utterance's audio window.
    last_sealed_end: f32,
    /// End of the last Apple segment admitted to committed canvas. Unlike the
    /// PCM retention cursor, this advances even when Layer 1 audio lookup is
    /// unavailable: Apple segment time is the authority for text disjointness.
    last_apple_segment_end: f32,
    /// Seals whose audio window could not be resolved (F3 falsification).
    unresolved_windows: u64,
    /// Whisper windows accepted onto the provider queue.
    windows_admitted: u64,
    /// Accepted windows built from more than one occurrence.
    windows_coalesced: u64,
    /// Windows refused before a provider ever saw them, by reason code.
    windows_refused_before_inference: BTreeMap<String, u64>,
    /// Seals where Layer 1 recovered speech it could not place on the canvas
    /// (W-C). A non-zero count means the stop path is owed a residual gap fill.
    under_commit_escalations: u64,
    /// Layer 1 hand-off, present only when layered transcription is armed.
    tail_patch: Option<mpsc::Sender<TailPatchRequest>>,
    /// Sealed fragments waiting to share one Whisper window (~5 segments).
    layer1_coalesce: Layer1Coalesce,
    refinement_pending: VecDeque<TailPatchRequest>,
    refinement_submitted: BTreeMap<(u64, u64, u64), TailPatchInFlight>,
    /// Exclusive Whisper text for an occurrence sliced across windows.
    /// Admitted once, when those slices partition the occurrence and every
    /// intersecting pin was an exclusive tail.
    whisper_slices: BTreeMap<OccurrenceIdentity, Vec<(u64, u64, String)>>,
    /// Occurrences whose whole-span replacement was already refused.
    whisper_span_refused: BTreeSet<OccurrenceIdentity>,
    refinement_clock: Instant,
    refinement_lane_lost: bool,
    refinement_started: Instant,
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
    pending_silero_words: BTreeMap<u64, Vec<FusionWord>>,
    unmatched_silero_words: Vec<FusionWord>,
    /// Session PCM identity, never text equality: repeated labels may be speech.
    warned_unmatched_words: BTreeSet<(u64, u64)>,
    no_time_overlap_warnings: u64,
    /// Silero only appends utterances and extends/closes its last range.
    /// Keep the last slice input so unchanged ticks do not clone/scan words.
    silero_slice_revision: Option<(usize, Option<super::silero_fusion::SileroUtterance>)>,
    reconciled_silero: BTreeSet<u64>,
    /// Shared one-throne ledger.
    acoustic_ledger: Arc<Mutex<AcousticLedger>>,
    /// Measured threshold frozen into the same settings snapshot. Absence is a
    /// fail-closed W2 state: no occurrence qualifies and no text mutates.
    energy_calibration: Option<EnergyCalibration>,
    /// This take's capture energy ladder, bound to session and capture epoch.
    /// The live writer is the async capture arm; this is its reader handle.
    capture_energy: CaptureEnergyOwner,
}

fn inflight_key(identity: &TailRequestIdentity) -> (u64, u64, u64) {
    (
        identity.request_id,
        identity.range.sample_start,
        identity.range.sample_end,
    )
}

/// One timed pin routed to a single open member.
#[derive(Clone)]
struct RoutedPin {
    index: usize,
    pin: OccurrenceIdentity,
    text: String,
}

/// Exclusive pins for one member, plus whether any other pin blocks the span.
#[derive(Clone, Default)]
struct MemberPinRoute {
    exclusive: Vec<RoutedPin>,
    blocked: bool,
}

fn exclusive_label(pins: &[RoutedPin]) -> String {
    let mut ordered = pins.to_vec();
    ordered.sort_by_key(|pin| pin.pin.sample_start);
    ordered
        .iter()
        .map(|pin| pin.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Exclusive windows cover the occurrence when they abut from its start to its end.
fn exclusive_slices_cover(occurrence: &OccurrenceIdentity, slices: &[(u64, u64, String)]) -> bool {
    let mut cursor = occurrence.sample_start;
    for (start, end, _) in slices {
        if *start != cursor || *end <= *start {
            return false;
        }
        cursor = *end;
    }
    cursor == occurrence.sample_end
}

fn pin_intersects(pin: &OccurrenceIdentity, member: &OccurrenceIdentity) -> bool {
    pin.sample_end > member.sample_start && pin.sample_start < member.sample_end
}

impl AppleSealState {
    fn emit_speech_integrity(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let debts = ledger.pending_text_recoveries(&self.session_id, self.capture_epoch);
        let recovering = debts.iter().any(|occurrence| {
            ledger.frontier_of(occurrence).is_some_and(|frontier| {
                frontier
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper)
            })
        });
        drop(ledger);
        let phase = if recovering {
            SpeechIntegrityPhase::Recovering
        } else if !debts.is_empty() {
            SpeechIntegrityPhase::Unresolved
        } else {
            self.speech_progress.phase(self.sample_rate)
        };
        let debt_ms = self.speech_progress.debt_ms(self.sample_rate);
        if self.last_integrity.as_ref().is_some_and(|previous| {
            previous.phase == phase
                && previous.pending_occurrences == debts.len() as u64
                && previous.acoustic_speech_ms_since_text_advance / 100 == debt_ms / 100
        }) {
            return;
        }
        let evidence = SpeechIntegrity {
            session_id: self.session_id.clone(),
            capture_epoch: self.capture_epoch,
            sequence: self
                .last_integrity
                .as_ref()
                .map_or(1, |previous| previous.sequence + 1),
            acoustic_speech_ms_since_text_advance: debt_ms,
            pending_occurrences: debts.len() as u64,
            phase,
        };
        self.last_integrity = Some(evidence.clone());
        let _ = ev_tx.send(EngineEvent::SpeechIntegrity { evidence });
    }

    fn observe_apple_progress(&mut self, text: &str, segments: &[TranscriptSegment]) {
        let word_end = segments
            .iter()
            .map(|segment| (segment.end_ts.max(0.0) * self.sample_rate as f32) as u64)
            .max()
            .unwrap_or(0);
        self.speech_progress.observe_apple(text, word_end);
    }

    fn window_by_samples(&self, start: u64, end: u64) -> Option<ResolvedAudioWindow> {
        if let Some(archive) = &self.terminal_pcm {
            archive.window(start, end)
        } else {
            self.audio
                .window_by_samples(start, end)
                .filter(|window| window.sample_start == start && window.sample_end == end)
        }
    }

    /// Fresh isolated seal state with Layer 1 disabled (`tail_patch: None`).
    /// Product-mode arming is injected by the session owner, not this test helper.
    #[cfg(any())]
    fn new(sample_rate: u32) -> Self {
        Self::new_for_session(sample_rate, uuid::Uuid::new_v4().to_string(), 0)
    }

    fn new_for_session(sample_rate: u32, session_id: String, capture_epoch: u64) -> Self {
        let session_id_for_energy = session_id.clone();
        let speech_progress = SpeechProgress::new(session_id.clone(), capture_epoch, sample_rate);
        let state = Self {
            session_id,
            capture_epoch,
            sample_rate,
            preview_rev: 0,
            speech_progress,
            last_integrity: None,
            utterance_id: 0,
            open_partial: String::new(),
            open_partial_segments: Vec::new(),
            sealed_count: 0,
            filtered_empty_drops: 0,
            audio: LiveAudioBuffer::new(sample_rate, DEFAULT_RETENTION_SECS),
            terminal_pcm: None,
            last_sealed_end: 0.0,
            last_apple_segment_end: 0.0,
            unresolved_windows: 0,
            windows_admitted: 0,
            windows_coalesced: 0,
            windows_refused_before_inference: BTreeMap::new(),
            under_commit_escalations: 0,
            tail_patch: None,
            layer1_coalesce: Layer1Coalesce::default(),
            refinement_pending: VecDeque::new(),
            refinement_submitted: BTreeMap::new(),
            whisper_slices: BTreeMap::new(),
            whisper_span_refused: BTreeSet::new(),
            refinement_clock: Instant::now(),
            refinement_lane_lost: false,
            refinement_started: Instant::now(),
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
            // One source of truth for the default cut. Without a Silero
            // ingress nothing reads this field; when one arms, `from_env`
            // resolves the same default unless an operator overrode it.
            fusion_context: FusionContextMode::default(),
            pending_silero_words: BTreeMap::new(),
            unmatched_silero_words: Vec::new(),
            warned_unmatched_words: BTreeSet::new(),
            no_time_overlap_warnings: 0,
            silero_slice_revision: None,
            reconciled_silero: BTreeSet::new(),
            acoustic_ledger: Arc::new(Mutex::new(AcousticLedger::new())),
            energy_calibration: None,
            capture_energy: CaptureEnergyOwner::bind(session_id_for_energy, capture_epoch),
        };
        state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bind_capture_rate(sample_rate);
        state
    }

    /// Adopt the capture arm's energy-ladder owner.
    ///
    /// The worker is the reader; the writer lives on the async capture arm.
    /// Installing its handle here is what makes the two threads share one
    /// measurement instead of two ladders that agree by coincidence.
    fn bind_capture_energy(&mut self, owner: CaptureEnergyOwner) {
        debug_assert!(
            owner
                .identity()
                .matches(&self.session_id, self.capture_epoch),
            "the capture owner must name this take"
        );
        self.capture_energy = owner;
    }

    fn new_for_session_with_ledger(
        sample_rate: u32,
        session_id: String,
        capture_epoch: u64,
        acoustic_ledger: Arc<Mutex<AcousticLedger>>,
        energy_calibration: Option<EnergyCalibration>,
    ) -> Self {
        acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bind_capture_rate(sample_rate);
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
        let utterance_id = piece.utterance_id;
        let occurrence = piece.occurrence.clone();
        if self
            .pending_events
            .get(&utterance_id)
            .is_none_or(|pending| pending.occurrence != occurrence)
        {
            return false;
        }
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ledger.is_qualified(&occurrence) || ledger.is_sealed(&occurrence) {
            return false;
        }
        let scheduled = if ledger.frontier_of(&occurrence).is_none() {
            ledger.schedule_frontier(occurrence.clone(), [LedgerObservationProducer::Whisper]);
            true
        } else {
            ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::Whisper)
        };
        drop(ledger);
        if !scheduled {
            return false;
        }
        self.refinement_receipt(&occurrence, "admitted");
        let limit = self.sample_rate.max(1) as usize * LIVE_REFINEMENT_PCM_SECS;
        if piece.audio.len() > limit || self.tail_patch.is_none() {
            let reason = if self.tail_patch.is_none() {
                RefinementFailure::LaneGone
            } else {
                RefinementFailure::BacklogExhausted
            };
            self.fail_refinement(ev_tx, utterance_id, &occurrence, reason);
            return false;
        }
        if self.layer1_coalesce.is_empty() {
            self.layer1_coalesce
                .set_neighbour(self.sealed_prefix.clone());
        }
        for flush in self
            .layer1_coalesce
            .push_at(piece, self.sample_rate, self.refinement_clock)
        {
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
        let identity = flush
            .member_occurrences
            .first()
            .map(|(_, occurrence)| occurrence);
        let valid = identity.is_some_and(|first| {
            first.session == self.session_id
                && first.capture_epoch == self.capture_epoch
                && flush.admit_sample_start >= flush.sample_start
                && flush.admit_sample_end <= flush.sample_end
                && flush.admit_sample_end > flush.admit_sample_start
                && flush.member_occurrences.iter().all(|(_, member)| {
                    member.same_capture(first)
                        && ((member.sample_start >= flush.sample_start
                            && member.sample_end <= flush.sample_end)
                            || (flush.member_occurrences.len() == 1
                                && member.sample_start <= flush.admit_sample_start
                                && member.sample_end >= flush.admit_sample_end))
                })
        }) && flush.member_occurrences.len() == flush.member_ids.len()
            && flush.sample_end > flush.sample_start
            && flush.audio.len() as u64 == flush.sample_end - flush.sample_start;
        if !valid {
            self.note_window_refused_before_inference(RefinementFailure::InvalidIdentity);
            for (id, occurrence) in &flush.member_occurrences {
                self.fail_refinement(ev_tx, *id, occurrence, RefinementFailure::InvalidIdentity);
            }
            return false;
        }
        let request = TailPatchRequest {
            utterance_id: flush.primary_utterance_id,
            committed_text: flush.committed_text,
            neighbour_context: flush.neighbour_context,
            audio: flush.audio,
            provider_request: TailProviderRequest {
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
            },
            member_occurrences: flush.member_occurrences,
            admit_sample_start: flush.admit_sample_start,
            admit_sample_end: flush.admit_sample_end,
        };
        self.retry_refinements(ev_tx);
        let pending_samples: usize = self
            .refinement_pending
            .iter()
            .map(|job| job.audio.len())
            .sum();
        if self.refinement_pending.len() >= LIVE_REFINEMENT_PENDING_CAP
            || pending_samples.saturating_add(request.audio.len())
                > self.sample_rate.max(1) as usize * LIVE_REFINEMENT_PCM_SECS
        {
            self.tail_patch_backpressure_drops =
                self.tail_patch_backpressure_drops.saturating_add(1);
            self.note_window_refused_before_inference(RefinementFailure::BacklogExhausted);
            for (id, occurrence) in &request.member_occurrences {
                self.fail_refinement(ev_tx, *id, occurrence, RefinementFailure::BacklogExhausted);
            }
            return false;
        }
        self.windows_admitted = self.windows_admitted.saturating_add(1);
        if request.member_occurrences.len() > 1 {
            self.windows_coalesced = self.windows_coalesced.saturating_add(1);
        }
        self.refinement_pending.push_back(request);
        self.retry_refinements(ev_tx);
        true
    }

    fn note_window_refused_before_inference(&mut self, reason: RefinementFailure) {
        *self
            .windows_refused_before_inference
            .entry(reason.code().to_string())
            .or_default() += 1;
    }

    fn session_conservation(&self) -> SessionConservationReceipt {
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        SessionConservationReceipt::from_ledger(
            &ledger,
            self.windows_admitted,
            self.windows_coalesced,
            self.unresolved_windows,
            self.windows_refused_before_inference.clone(),
        )
    }

    fn refinement_receipt(&self, occurrence: &OccurrenceIdentity, disposition: &'static str) {
        info!(
            session = %occurrence.session,
            capture_epoch = occurrence.capture_epoch,
            sample_start = occurrence.sample_start,
            sample_end = occurrence.sample_end,
            elapsed_ms = self.refinement_clock.saturating_duration_since(self.refinement_started).as_millis() as u64,
            disposition,
            "live_refinement"
        );
    }

    fn fail_refinement(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        id: u64,
        occurrence: &OccurrenceIdentity,
        reason: RefinementFailure,
    ) {
        self.refinement_receipt(occurrence, reason.code());
        let _ = ev_tx.send(EngineEvent::Warning {
            code: reason.code().into(),
            message: format!(
                "session={} epoch={} occurrence={} samples={}..{} refinement_not_completed=true",
                occurrence.session,
                occurrence.capture_epoch,
                id,
                occurrence.sample_start,
                occurrence.sample_end,
            ),
        });
        self.return_whisper_without_label(ev_tx, id, occurrence);
        self.emit_pending_seal(ev_tx, id);
    }

    /// One bounded nonblocking attempt per pending request, stopping at pressure.
    fn retry_refinements(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        while let Some(request) = self.refinement_pending.pop_front() {
            let Some(sender) = self.tail_patch.as_ref() else {
                for (id, occurrence) in &request.member_occurrences {
                    self.fail_refinement(ev_tx, *id, occurrence, RefinementFailure::LaneGone);
                }
                continue;
            };
            let inflight = TailPatchInFlight {
                utterance_id: request.utterance_id,
                request_identity: request.provider_request.identity.clone(),
                admit_sample_start: request.admit_sample_start,
                admit_sample_end: request.admit_sample_end,
                member_occurrences: request.member_occurrences.clone(),
            };
            match sender.try_send(request) {
                Ok(()) => {
                    for (_, occurrence) in &inflight.member_occurrences {
                        self.refinement_receipt(occurrence, "submitted");
                    }
                    let key = inflight_key(&inflight.request_identity);
                    self.refinement_submitted.insert(key, inflight);
                    self.tail_patch_awaiting_completion =
                        self.tail_patch_awaiting_completion.saturating_add(1);
                }
                Err(mpsc::error::TrySendError::Full(request)) => {
                    self.refinement_pending.push_front(request);
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(request)) => {
                    self.tail_patch = None;
                    self.refinement_lane_lost = true;
                    for (id, occurrence) in &request.member_occurrences {
                        self.fail_refinement(ev_tx, *id, occurrence, RefinementFailure::LaneGone);
                    }
                }
            }
        }
    }

    /// Called on every live worker turn, independent of Apple engine lifecycle.
    fn tick_refinements(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>, now: Instant) {
        self.refinement_clock = now;
        for flush in self.layer1_coalesce.flush_due(now) {
            self.queue_layer1_flush(ev_tx, flush);
        }
        self.retry_refinements(ev_tx);
    }

    /// Return true while Stop still owns work. One absolute deadline applies
    /// to both pending transport and submitted jobs; completion races are
    /// resolved by the same registered request identity as the live path.
    fn stop_refinements_tick(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        now: Instant,
        deadline: Instant,
    ) -> bool {
        if now >= deadline {
            self.refinement_clock = now;
            self.return_outstanding_whisper_without_label(ev_tx);
            return false;
        }
        self.tick_refinements(ev_tx, now);
        self.tail_patch_awaiting_completion > 0 || !self.refinement_pending.is_empty()
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

    /// Send every pin the admit filter used to drop. Exclusive-tail pins stay
    /// with their member; covered overlap is a named refusal; the rest stays
    /// visible at its own PCM range and does not enter the committed map.
    /// A pin that intersects a member without being its exclusive tail blocks
    /// replacement of that whole member.
    fn route_overlap_pins(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        request_id: u64,
        admit_sample_start: u64,
        admit_sample_end: u64,
        members: &[(u64, OccurrenceIdentity)],
        segments: &[TimedTailSegment],
    ) -> Vec<MemberPinRoute> {
        let open_members = members
            .iter()
            .map(|(_, occurrence)| occurrence.clone())
            .collect::<Vec<_>>();
        let mut routes = vec![MemberPinRoute::default(); members.len()];
        let mut side = Vec::new();
        {
            let ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (index, segment) in segments.iter().enumerate() {
                let text = segment.text.trim();
                if text.is_empty() {
                    continue;
                }
                let pin = OccurrenceIdentity::from(&segment.range);
                match ledger.classify_overlap_pin(
                    &pin,
                    admit_sample_start,
                    admit_sample_end,
                    &open_members,
                ) {
                    OverlapPinClass::ExclusiveTail { member_index } => {
                        routes[member_index].exclusive.push(RoutedPin {
                            index,
                            pin,
                            text: text.to_string(),
                        });
                    }
                    OverlapPinClass::Replay => {
                        // A replayed range is already covered and lies outside this
                        // window's admit. Refuse that pin. It does not veto the
                        // exclusive remainder: a straddle (unanchored) still does.
                        side.push((index, pin, text.to_string(), None));
                    }
                    OverlapPinClass::Unanchored(reason) => {
                        for (member_index, member) in open_members.iter().enumerate() {
                            if pin_intersects(&pin, member) {
                                routes[member_index].blocked = true;
                            }
                        }
                        side.push((index, pin, text.to_string(), Some(reason)));
                    }
                }
            }
        }
        for (index, pin, text, unanchored) in side {
            let observation = LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                request_id,
                1_000 + index as u64,
                pin,
            );
            let receipt = {
                let mut ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match unanchored {
                    None => ledger.refuse_replayed_range(&observation, &text),
                    Some(reason) => ledger.keep_visible_unanchored(&observation, &text, reason),
                }
            };
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation,
                label: text,
                receipt,
            });
        }
        routes
    }

    fn keep_routed_visible(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        request_id: u64,
        pins: &[RoutedPin],
    ) {
        for pin in pins {
            let observation = LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                request_id,
                1_000 + pin.index as u64,
                pin.pin.clone(),
            );
            let receipt = {
                let mut ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ledger.keep_visible_unanchored(
                    &observation,
                    &pin.text,
                    NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
                )
            };
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation,
                label: pin.text.clone(),
                receipt,
            });
        }
    }

    fn emit_span_refusal(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        request_id: u64,
        occurrence: &OccurrenceIdentity,
        label: &str,
        reason: RefuseReason,
    ) {
        let observation = LedgerObservationIdentity::new(
            LedgerObservationProducer::Whisper,
            request_id,
            10_000,
            occurrence.clone(),
        );
        let receipt = {
            let mut ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.refuse_replacement(&observation, label, reason)
        };
        let _ = ev_tx.send(EngineEvent::LedgerMutation {
            observation,
            label: label.to_string(),
            receipt,
        });
    }

    /// Stop with an open slice accumulator writes no partial label. One named
    /// receipt per pending occurrence; the slice texts are already visible.
    fn refuse_incomplete_whisper_spans(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        let pending = std::mem::take(&mut self.whisper_slices);
        for (occurrence, slices) in pending {
            if !self.whisper_span_refused.insert(occurrence.clone()) {
                continue;
            }
            let label = slices
                .iter()
                .map(|(_, _, text)| text.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            self.emit_span_refusal(
                ev_tx,
                0,
                &occurrence,
                &label,
                RefuseReason::IncompleteExclusiveCoverage,
            );
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
        let job_key = request_identity.as_ref().map(inflight_key);
        let matched = job_key.as_ref().is_some_and(|key| {
            self.refinement_submitted.get(key).is_some_and(|job| {
                job.utterance_id == utterance_id
                    && request_identity.as_ref() == Some(&job.request_identity)
                    && member_occurrences == job.member_occurrences
            })
        });
        if !matched {
            return;
        }
        let job = self
            .refinement_submitted
            .remove(&job_key.expect("matched key"))
            .expect("matched job");
        let admit_sample_start = job.admit_sample_start;
        let admit_sample_end = job.admit_sample_end;
        self.tail_patch_awaiting_completion = self.tail_patch_awaiting_completion.saturating_sub(1);
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
        let segments = payload
            .as_ref()
            .map(|payload| payload.segments.as_slice())
            .unwrap_or(&[]);
        let routes = self.route_overlap_pins(
            ev_tx,
            request_id,
            admit_sample_start,
            admit_sample_end,
            &exact_open_members,
            segments,
        );
        let mut mutation_admitted = false;
        for (generation, (member_id, occurrence)) in exact_open_members.iter().enumerate() {
            let had_debt = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .text_recovery_pending(occurrence);
            let route = routes.get(generation).cloned().unwrap_or_default();
            let exclusive_text = exclusive_label(&route.exclusive);
            let sliced = occurrence.sample_start < admit_sample_start
                || occurrence.sample_end > admit_sample_end;
            if self.whisper_span_refused.contains(occurrence) {
                self.keep_routed_visible(ev_tx, request_id, &route.exclusive);
                self.refinement_receipt(occurrence, "sliced");
                continue;
            }
            if route.blocked {
                self.whisper_span_refused.insert(occurrence.clone());
                self.whisper_slices.remove(occurrence);
                self.keep_routed_visible(ev_tx, request_id, &route.exclusive);
                self.emit_span_refusal(
                    ev_tx,
                    request_id,
                    occurrence,
                    &exclusive_text,
                    RefuseReason::IntersectingPinNotExclusive,
                );
                if sliced {
                    self.refinement_receipt(occurrence, "sliced");
                    continue;
                }
            }
            // A rejected pin cannot be laundered through whole-window text.
            // A blocked member that this window already covers wholly closes
            // with no new label: the straddle veto stands, and the observer
            // still returns.
            let mut label = if route.blocked {
                None
            } else if !exclusive_text.is_empty() {
                Some(exclusive_text)
            } else if payload.as_ref().is_some_and(|payload| {
                payload.segments.is_empty()
                    && single_member
                    && &OccurrenceIdentity::from(&payload.identity.range) == occurrence
                    && !payload.text.trim().is_empty()
            }) {
                payload
                    .as_ref()
                    .map(|payload| payload.text.trim().to_string())
            } else {
                None
            };
            if sliced {
                if let Some(text) = label.clone() {
                    let start = admit_sample_start.max(occurrence.sample_start);
                    let end = admit_sample_end.min(occurrence.sample_end);
                    if end > start {
                        let slices = self.whisper_slices.entry(occurrence.clone()).or_default();
                        slices.push((start, end, text));
                        slices.sort_by_key(|(start, _, _)| *start);
                        slices.dedup_by_key(|(start, end, _)| (*start, *end));
                    }
                }
                self.keep_routed_visible(ev_tx, request_id, &route.exclusive);
                let covered = self
                    .whisper_slices
                    .get(occurrence)
                    .is_some_and(|slices| exclusive_slices_cover(occurrence, slices));
                if !covered {
                    self.refinement_receipt(occurrence, "sliced");
                    continue;
                }
                let joined = self
                    .whisper_slices
                    .remove(occurrence)
                    .map(|slices| {
                        slices
                            .iter()
                            .map(|(_, _, text)| text.trim())
                            .filter(|text| !text.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                if joined.is_empty() {
                    self.refinement_receipt(occurrence, "sliced");
                    continue;
                }
                label = Some(joined);
            }
            let no_label = label.as_ref().is_none_or(|text| text.trim().is_empty());
            match admit_ledger_label(
                self,
                ev_tx,
                LabelAdmission {
                    observation: LedgerObservationIdentity::new(
                        LedgerObservationProducer::Whisper,
                        request_id,
                        generation as u64,
                        occurrence.clone(),
                    ),
                    label: label.as_deref().unwrap_or(""),
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            ) {
                Some(receipt) => {
                    mutation_admitted |= receipt.grants_mutation();
                    if matches!(
                        receipt,
                        MutationReceipt::Refuse { .. }
                            | MutationReceipt::KeepVisibleUnanchored { .. }
                    ) {
                        let reason = if no_label {
                            RefinementFailure::NoLabel
                        } else {
                            RefinementFailure::InvalidIdentity
                        };
                        self.fail_refinement(ev_tx, *member_id, occurrence, reason);
                    } else {
                        // The reducer's committed label also supplies final telemetry;
                        // a Whisper-first occurrence has no earlier Apple baseline.
                        let label = self
                            .acoustic_ledger
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .text_of(occurrence)
                            .map(str::to_owned);
                        if let Some(label) = label
                            && let Some(pending) = self.pending_events.get_mut(member_id)
                        {
                            pending.layer1_baseline = label;
                        }
                        self.refinement_receipt(occurrence, "completed");
                    }
                }
                None => {
                    self.fail_refinement(ev_tx, *member_id, occurrence, RefinementFailure::NoLabel)
                }
            }
            let debt_resolved = had_debt
                && !self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .text_recovery_pending(occurrence);
            if debt_resolved && let Some(fusion) = self.fusion.as_ref() {
                self.speech_progress
                    .recovered_occurrence(occurrence, &fusion.acoustic_speech_evidence());
            }
            self.emit_pending_seal(ev_tx, *member_id);
        }
        if mutation_admitted {
            self.tail_patch_jobs_applied = self.tail_patch_jobs_applied.saturating_add(1);
            self.tail_patch_replacements = self.tail_patch_replacements.saturating_add(1);
        } else {
            self.tail_patch_jobs_skipped = self.tail_patch_jobs_skipped.saturating_add(1);
        }
    }

    /// Return one launched Whisper slot with an explicit no-label receipt.
    fn return_whisper_without_label(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        utterance_id: u64,
        occurrence: &OccurrenceIdentity,
    ) {
        let formatter = self.formatter.clone();
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ledger.frontier_of(occurrence).is_some_and(|frontier| {
            frontier
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        }) {
            return;
        }
        let observation = LedgerObservationIdentity::new(
            LedgerObservationProducer::Whisper,
            utterance_id,
            0,
            occurrence.clone(),
        );
        let receipt = ledger.admit(&observation, "");
        let _ = ev_tx.send(EngineEvent::LedgerMutation {
            observation,
            label: String::new(),
            receipt,
        });
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
        self.refuse_incomplete_whisper_spans(ev_tx);
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
            let id = self
                .pending_events
                .iter()
                .find_map(|(id, pending)| (pending.occurrence == occurrence).then_some(*id))
                .unwrap_or(0);
            self.fail_refinement(ev_tx, id, &occurrence, RefinementFailure::StopDeadline);
        }
        self.refinement_pending.clear();
        self.refinement_submitted.clear();
        self.layer1_coalesce.force_flush();
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
    /// Ledger and window census read at worker exit. Not derived from job buckets.
    conservation: SessionConservationReceipt,
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
                grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
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
    let Some(fusion) = state.fusion.as_ref() else {
        return false;
    };
    let utterances = fusion.ledger().utterances();
    if disjoint.is_empty()
        && state
            .silero_slice_revision
            .as_ref()
            .is_some_and(|(len, last)| {
                *len == utterances.len() && last.as_ref() == utterances.last()
            })
    {
        return true;
    }
    // Check before cloning the ledger or converting/copying retained words.
    // A newly opened, extended or closed range must still retry the leftovers
    // and reconcile pending words, including the terminal flush.
    state.silero_slice_revision = Some((utterances.len(), utterances.last().cloned()));
    let ledger = fusion.ledger().clone();
    reconcile_silero_ledger(state, ev_tx, &ledger, disjoint)
}

/// PCM each closed utterance owns once boundary overlaps are given to the
/// tightest window. Context pads stay on the Silero range; they are not a
/// second physical identity over the same samples.
fn exclusive_closed_spans(
    utterances: &[super::silero_fusion::SileroUtterance],
) -> BTreeMap<u64, (u64, u64)> {
    let closed: Vec<_> = utterances
        .iter()
        .filter(|utterance| utterance.closed)
        .collect();
    let mut points = Vec::with_capacity(closed.len().saturating_mul(2));
    for utterance in &closed {
        points.push(utterance.range.sample_start);
        points.push(utterance.range.sample_end);
    }
    points.sort_unstable();
    points.dedup();
    let mut pieces: BTreeMap<u64, Vec<(u64, u64)>> = BTreeMap::new();
    for pair in points.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if end <= start {
            continue;
        }
        let owner = closed
            .iter()
            .filter(|utterance| {
                utterance.range.sample_start <= start && end <= utterance.range.sample_end
            })
            .min_by_key(|utterance| {
                (
                    utterance
                        .range
                        .sample_end
                        .saturating_sub(utterance.range.sample_start),
                    utterance.range.sample_start,
                    utterance.id,
                )
            });
        if let Some(owner) = owner {
            pieces.entry(owner.id).or_default().push((start, end));
        }
    }
    let mut spans = BTreeMap::new();
    for utterance in &closed {
        let Some(parts) = pieces.get(&utterance.id) else {
            continue;
        };
        let start = parts[0].0;
        let mut end = parts[0].1;
        let mut contiguous = true;
        for (part_start, part_end) in parts.iter().skip(1) {
            if *part_start != end {
                contiguous = false;
                break;
            }
            end = *part_end;
        }
        if contiguous && end > start {
            spans.insert(utterance.id, (start, end));
        } else {
            spans.insert(
                utterance.id,
                (utterance.range.sample_start, utterance.range.sample_end),
            );
        }
    }
    spans
}

/// Production reconciliation seam; tests supply physical edges without a model.
fn reconcile_silero_ledger(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    ledger: &super::silero_fusion::UtteranceLedger,
    disjoint: &[TranscriptSegment],
) -> bool {
    if ledger.utterances().iter().any(|utterance| {
        utterance.range.session != state.session_id
            || utterance.range.capture_epoch != state.capture_epoch
    }) {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: RefinementFailure::InvalidIdentity.code().into(),
            message: "Silero evidence belongs to another capture; current occurrence unchanged"
                .into(),
        });
        return false;
    }
    let apple_words = apple_segments_on_pcm_clock(state, disjoint);
    let mut fusion_words = std::mem::take(&mut state.unmatched_silero_words);
    fusion_words.extend(apple_words.iter().map(FusionWord::from_timed));
    let (sliced, leftover) = slice_apple_words(ledger, &fusion_words);
    let mut newly_unmatched = 0;
    for word in &leftover {
        if state
            .warned_unmatched_words
            .insert((word.sample_start, word.sample_end))
        {
            newly_unmatched += 1;
        }
    }
    if newly_unmatched > 0 {
        state.no_time_overlap_warnings += 1;
        let _ = ev_tx.send(EngineEvent::Warning {
            code: SkipReasonCode::NoTimeOverlap.as_str().to_string(),
            message: format!(
                "apple leftover_words={} new_unmatched_words={} sliced_utterances={}",
                leftover.len(),
                newly_unmatched,
                sliced.len()
            ),
        });
    }
    state.unmatched_silero_words = leftover;

    let rate = state.sample_rate.max(1) as f32;
    let context = state.fusion_context;
    let pad_secs = match context {
        FusionContextMode::SymmetricPad => super::silero_fusion::DEFAULT_SYMMETRIC_PAD_SECS,
        _ => super::silero_fusion::DEFAULT_LEFT_PAD_SECS,
    };
    let pad_samples = (pad_secs * rate).round() as u64;
    let long_silence = (super::silero_fusion::LONG_SILENCE_FENCE_SECS * rate).round() as u64;

    // Candidate labels remain paint until the physical extent closes. Retain
    // every observation until reconciliation; equality of text is irrelevant.
    for (id, words) in sliced {
        if !state.reconciled_silero.contains(&id) {
            state
                .pending_silero_words
                .entry(id)
                .or_default()
                .extend(words);
        }
    }
    // Overlapping Silero windows are one physical claim per sample. Words move
    // with their midpoint onto the tightest closed span before any occurrence
    // is minted, so a pad shared with a neighbour cannot erase the word.
    let exclusive = exclusive_closed_spans(ledger.utterances());
    if !exclusive.is_empty() {
        let mut held = Vec::new();
        let closed_ids: Vec<u64> = ledger
            .utterances()
            .iter()
            .filter(|utterance| utterance.closed)
            .map(|utterance| utterance.id)
            .collect();
        for id in closed_ids {
            if let Some(words) = state.pending_silero_words.remove(&id) {
                held.extend(words);
            }
        }
        for word in held {
            let mid = word.sample_start + word.sample_end.saturating_sub(word.sample_start) / 2;
            let owner = exclusive
                .iter()
                .find_map(|(&id, &(start, end))| (start <= mid && mid < end).then_some(id));
            match owner {
                Some(id) if !state.reconciled_silero.contains(&id) => {
                    state.pending_silero_words.entry(id).or_default().push(word);
                }
                Some(_) => {}
                None => state.unmatched_silero_words.push(word),
            }
        }
    }
    for silero in ledger
        .utterances()
        .iter()
        .filter(|utterance| utterance.closed)
    {
        let utterance_id = silero.id;
        if state.reconciled_silero.contains(&utterance_id) {
            continue;
        }
        let candidates = state
            .pending_silero_words
            .remove(&utterance_id)
            .unwrap_or_default();
        // Replayed exact word coordinates revise that word, not a second
        // acoustic occurrence. Disjoint equal words survive independently.
        let mut by_range = BTreeMap::new();
        for word in candidates {
            by_range.insert((word.sample_start, word.sample_end), word);
        }
        let mut words = by_range.into_values().collect::<Vec<_>>();
        if words
            .windows(2)
            .any(|pair| pair[0].sample_end > pair[1].sample_start)
        {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "apple_closed_occurrence_ambiguous_word_ranges".into(),
                message: format!("utterance={utterance_id} requires fresh exact-PCM evidence"),
            });
            words.clear();
        }
        let text = words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // Blank Apple evidence neither consumes this identity nor blocks its PCM.
        let has_apple_label = !text.trim().is_empty();
        if !has_apple_label && state.tail_patch.is_none() && !state.refinement_lane_lost {
            // An explicitly Apple-only take has no recovery observer to launch.
            // Leave the occurrence unlabelled until Apple actually observes it;
            // absent text remains uncovered rather than an invented empty seal.
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

        // Silero has already selected the physical occurrence. The identity is
        // the exclusive PCM, not the pad it shares with a neighbour. Admit the
        // slice-local Apple label before the raw final can escape as telemetry.
        // There is no independent slice-local Lexicon rewrite on this path, so
        // Lexicon reports a no-change observation for the same exact label and
        // range. Callback-wide text is never copied across sliced occurrences.
        let (owned_start, owned_end) = match exclusive.get(&utterance_id) {
            Some(&(start, end)) if end > start => (start, end),
            _ => {
                let swallowed = exclusive.iter().any(|(&id, &(start, end))| {
                    id != utterance_id
                        && start <= silero.range.sample_start
                        && silero.range.sample_end <= end
                });
                if swallowed {
                    state.reconciled_silero.insert(utterance_id);
                    continue;
                }
                (silero.range.sample_start, silero.range.sample_end)
            }
        };
        let occurrence = OccurrenceIdentity::new(
            state.session_id.clone(),
            state.capture_epoch,
            owned_start,
            owned_end,
        );
        let first_attempt = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .frontier_of(&occurrence)
            .is_none();
        if first_attempt {
            state.refinement_receipt(&occurrence, "closed");
        }
        if !qualify_owned_occurrence(state, &occurrence) {
            let reason = if state
                .window_by_samples(occurrence.sample_start, occurrence.sample_end)
                .is_none()
            {
                RefinementFailure::PcmUnavailable
            } else {
                RefinementFailure::QualificationRefused
            };
            state.fail_refinement(ev_tx, utterance_id, &occurrence, reason);
            state.reconciled_silero.insert(utterance_id);
            continue;
        }
        if !has_apple_label && !first_attempt {
            continue;
        }
        let owes_recovery = !has_apple_label
            || state.fusion.as_ref().is_some_and(|fusion| {
                state.speech_progress.occurrence_has_debt(
                    &occurrence,
                    &fusion.acoustic_speech_evidence(),
                    state.sample_rate,
                )
            });
        if owes_recovery {
            state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .require_text_recovery(&occurrence);
            // Debt is a whole-occurrence job, never a coalesced tail sharing
            // another occurrence's result. Older pieces keep their own jobs.
            state.flush_layer1_coalesce(ev_tx);
        }
        if has_apple_label {
            let mut ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if ledger.is_sealed(&occurrence) {
                // Successful seals stay immutable; retain the refusal evidence.
                let observation = LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    utterance_id,
                    0,
                    occurrence.clone(),
                );
                let receipt = ledger.admit(&observation, &text);
                let _ = ev_tx.send(EngineEvent::LedgerMutation {
                    observation,
                    label: text.clone(),
                    receipt,
                });
                drop(ledger);
                state.reconciled_silero.insert(utterance_id);
                continue;
            }
            // A completed Whisper job without a label did not seal lexical
            // truth. Extend its accounting only with the real words now held;
            // the ledger preserves prior returns and rejects producer replay.
            let observation = LedgerObservationIdentity::new(
                LedgerObservationProducer::Apple,
                utterance_id,
                0,
                occurrence.clone(),
            );
            if !ledger.schedule_late_apple_label(&observation, &text)
                && ledger.frontier_of(&occurrence).is_some()
            {
                ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::Apple);
                ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::Lexicon);
            }
            state.reconciled_silero.insert(utterance_id);
        }
        let apple_admitted = if has_apple_label {
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
                    label: &text,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
        } else {
            None
        };
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

        let previous = ledger
            .utterances()
            .iter()
            .rev()
            .find(|prev| prev.closed && prev.range.sample_end <= silero.range.sample_start);
        let fence = previous
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
        let bounds = ContextBounds {
            long_silence_fence: fence,
            capture_end: state.audio.session_sample_end(),
            next_utterance_start: ledger
                .utterances()
                .iter()
                .find(|next| next.range.sample_start >= silero.range.sample_end)
                .map(|next| next.range.sample_start),
            previous_utterance_end: previous.map(|prev| prev.range.sample_end),
        };
        let request_range = bound_context_range(&silero.range, context, pad_samples, &bounds);
        // Context is an improvement, never a precondition. `window_by_samples`
        // refuses a range it cannot serve exactly, so a padded window whose
        // edges fell off retention (or landed past the archive) must fall back
        // to the occurrence's own span rather than costing Layer 1 the job
        // entirely. Ownership is unaffected either way — `occurrence` above was
        // minted from `silero.range`, not from this window.
        let window = if owes_recovery {
            state.window_by_samples(silero.range.sample_start, silero.range.sample_end)
        } else {
            state
                .window_by_samples(request_range.sample_start, request_range.sample_end)
                .or_else(|| {
                    state.window_by_samples(silero.range.sample_start, silero.range.sample_end)
                })
        };
        let _current_piece_owned = if let Some(window) = window {
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
            state.fail_refinement(
                ev_tx,
                utterance_id,
                &occurrence,
                RefinementFailure::PcmUnavailable,
            );
            false
        };
        if owes_recovery {
            state.flush_layer1_coalesce(ev_tx);
        }
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

/// Qualification consumes owned PCM and calibration, never a candidate label.
fn qualify_owned_occurrence(state: &AppleSealState, occurrence: &OccurrenceIdentity) -> bool {
    if occurrence.session != state.session_id || occurrence.capture_epoch != state.capture_epoch {
        return false;
    }
    let Some(calibration) = state.energy_calibration.as_ref() else {
        return false;
    };
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if ledger.is_qualified(occurrence) {
        return true;
    }
    let Some(window) = state.window_by_samples(occurrence.sample_start, occurrence.sample_end)
    else {
        return false;
    };
    if window.samples.is_empty() {
        return false;
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
        return false;
    }
    true
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
    if occurrence.session != state.session_id || occurrence.capture_epoch != state.capture_epoch {
        return None;
    }
    if matches!(
        energy,
        EnergyAdmission::QualifyFromOwnedPcm | EnergyAdmission::QualifyFinalPassGap
    ) && !qualify_owned_occurrence(state, &occurrence)
    {
        return None;
    }
    let formatter = state.formatter.clone();
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !ledger.is_qualified(&occurrence) {
        return None;
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

/// Admit only whole-session final-pass segments that fit wholly inside one
/// material uncovered speech range.
///
/// The segment's own PCM range becomes the occurrence. A segment that crosses
/// a coverage boundary is ambiguous: some of its words may already belong to
/// a committed Apple occurrence, so admitting its text against the whole gap
/// would duplicate speech. Such a segment is diagnostic evidence only and the
/// terminal coverage remains incomplete.
fn admit_full_pass_gap_segments(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    full_pass: &TailProviderPayload,
    uncovered_speech_ranges: &[TailSampleRange],
    threshold_samples: u64,
    gap_energy: EnergyAdmission,
) -> usize {
    let exact_timing = full_pass.evidence.timing_quality
        == crate::stt::tail_provider::TailTimingQuality::ExactSampleRange
        || (cfg!(test)
            && full_pass.evidence.timing_quality
                == crate::stt::tail_provider::TailTimingQuality::Synthetic);
    if !exact_timing || full_pass.validate().is_err() {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "seal_coverage_untrusted_segment_clock".into(),
            message: "gap evidence requires valid source-PCM segment coordinates".into(),
        });
        return 0;
    }
    let material_gaps = uncovered_speech_ranges
        .iter()
        .filter(|range| range.sample_end.saturating_sub(range.sample_start) > threshold_samples)
        .collect::<Vec<_>>();
    let mut admitted = 0usize;
    let mut contained = 0usize;

    for (generation, segment) in full_pass.segments.iter().enumerate() {
        let label = segment.text.trim();
        if label.is_empty() {
            continue;
        }
        let Some(_) = material_gaps
            .iter()
            .find(|gap| gap.contains(&segment.range))
        else {
            if material_gaps.iter().any(|gap| gap.overlaps(&segment.range)) {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "seal_coverage_full_pass_segment_straddles_gap".to_string(),
                    message: format!(
                        "segment {}..{} crosses a committed/uncovered PCM boundary",
                        segment.range.sample_start, segment.range.sample_end
                    ),
                });
            }
            continue;
        };
        contained = contained.saturating_add(1);

        let observation = LedgerObservationIdentity::new(
            LedgerObservationProducer::Whisper,
            full_pass.identity.request_id,
            generation as u64,
            OccurrenceIdentity::from(&segment.range),
        );
        if admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation,
                label,
                energy: gap_energy,
            },
        )
        .is_some_and(|receipt| receipt.grants_mutation())
        {
            admitted = admitted.saturating_add(1);
        }
    }

    if admitted == 0 && contained == 0 && !material_gaps.is_empty() {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "seal_coverage_full_pass_gap_unresolved".to_string(),
            message: "material gap had no source-mapped segment wholly contained in it".to_string(),
        });
    }
    admitted
}

/// Padded fusion ownership windows, summed. Diagnostics only: this is the
/// number that read a whole archived take as 99.5% speech.
fn fusion_utterance_ranges(state: &AppleSealState) -> Vec<TailSampleRange> {
    state
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
        .unwrap_or_default()
}

/// Choose the acoustic observer: the narrowest one that actually measured.
///
/// Silero crossings first — a set that exists is proof the VAD ran and heard
/// speech, which is the narrowest honest answer. Otherwise the capture energy
/// ladder, the other authenticated observer over the same PCM. Padded fusion
/// ownership windows are no longer a candidate: they stay open across pauses
/// and close on the capture cursor, so they measure ownership, never speech.
///
/// When neither observer measured, the answer is the unavailable evidence the
/// capture owner itself reports. Absence keeps its own name here instead of
/// arriving at the ledger as an empty set.
fn energy_lookup_without_voiced_hop(state: &AppleSealState) -> bool {
    let energy = state.capture_energy.session_active_speech_ranges(
        &state.session_id,
        state.capture_epoch,
        state.sample_rate,
    );
    matches!(energy.availability(), AcousticAvailability::Observed { .. })
        && energy.ranges().is_empty()
}

fn coverage_speech_evidence(state: &AppleSealState) -> AcousticSpeechEvidence {
    let captured_samples = state.audio.session_sample_end();
    let capture_energy = state.capture_energy.session_active_speech_ranges(
        &state.session_id,
        state.capture_epoch,
        state.sample_rate,
    );
    // The capture writer adjudicates the PCM it wrote. When it measured samples
    // it could not read, no later observer over the same buffer may certify
    // them: a downstream reader sees the identical NaN as a probability that
    // fails every threshold, which is indistinguishable from silence.
    if matches!(
        capture_energy.availability(),
        AcousticAvailability::InvalidMeasurement { .. }
    ) {
        return capture_energy;
    }
    if let Some(fusion) = state.fusion.as_ref() {
        let acoustic = fusion.acoustic_speech_evidence();
        if acoustic.observed_speech() {
            return within_capture(acoustic, captured_samples);
        }
    }
    within_capture(capture_energy, captured_samples)
}

/// Hold an observer's measured extent against the capture the take produced.
///
/// An observer reports how much PCM reached *it*. Nothing in the pipeline
/// guarantees that equals what the microphone produced — a chunk lane that
/// stops forwarding leaves no hole to detect, because the extent simply ends
/// early. The ledger compares committed and measured spans against the extent
/// the observer claims, so without this the unheard remainder was certified
/// covered whenever no word happened to be committed inside it.
///
/// `state.audio` is the capture owner's own count of samples seen this
/// session — retained or evicted — so this compares two authenticated facts.
/// It only ever downgrades: a short observer becomes unavailable, and no
/// evidence is promoted.
fn within_capture(
    evidence: AcousticSpeechEvidence,
    captured_samples: u64,
) -> AcousticSpeechEvidence {
    let Some(observed_samples) = evidence.availability().observed_samples() else {
        return evidence;
    };
    if observed_samples >= captured_samples {
        return evidence;
    }
    AcousticSpeechEvidence::unavailable(
        evidence.identity().clone(),
        evidence.producer(),
        AcousticAvailability::Discontinuous { observed_samples },
    )
}

fn publish_terminal_coverage(
    state: &AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
) -> SealCoverageReceipt {
    let speech = coverage_speech_evidence(state);
    let speech_samples = sum_range_samples(speech.ranges());
    let utterance_ranges = fusion_utterance_ranges(state);

    // Both numbers on one line, text-free. The acoustic set is what the receipt
    // was measured against; the utterance sum is what the receipt used to be
    // measured against, kept visible so a regression on this lane is one grep
    // away instead of a re-derivation from a 50-minute take.
    info!(
        session_id = %state.session_id,
        capture_epoch = state.capture_epoch,
        producer = speech.producer(),
        availability = speech.availability().as_str(),
        observed_samples = speech.availability().observed_samples(),
        speech_ranges = speech.ranges().len(),
        speech_samples,
        utterance_ranges = utterance_ranges.len(),
        utterance_samples = sum_range_samples(&utterance_ranges),
        capture_samples = state.audio.session_sample_end(),
        "seal_speech_set"
    );

    let no_voiced_hop = energy_lookup_without_voiced_hop(state);
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if no_voiced_hop {
        ledger.note_energy_lookup_without_voiced_hop();
    }
    let receipt = ledger.assess_seal_coverage(
        &state.session_id,
        state.capture_epoch,
        &speech,
        u64::from(state.sample_rate) * SEAL_COVERAGE_INCOMPLETE_MS / 1_000,
    );
    ledger.record_seal_coverage(receipt.clone());
    let _ = ev_tx.send(EngineEvent::SealCoverage {
        receipt: receipt.clone(),
        comparison: None,
    });
    receipt
}

/// Total samples described by a set of ranges, without merging overlaps.
/// Diagnostics arithmetic only — the receipt does its own merge.
fn sum_range_samples(ranges: &[TailSampleRange]) -> u64 {
    ranges
        .iter()
        .map(|range| range.sample_end.saturating_sub(range.sample_start))
        .sum()
}

fn drain_formatter_observers(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    done: &std_mpsc::Receiver<FormatterCompletion>,
) -> Result<()> {
    while state.formatter_awaiting_completion > 0 {
        let completion = done.recv().map_err(|error| {
            anyhow::anyhow!(
                "formatter completion channel closed with outstanding occurrences: {error}"
            )
        })?;
        anyhow::ensure!(
            state.complete_formatter(ev_tx, completion),
            "formatter completion did not return its emitter-sealed exact occurrence"
        );
    }
    Ok(())
}

/// One provider attempt for a single requested PCM range.
enum StopRangeAttempt {
    Ready(TailProviderPayload),
    MissingPcm,
    ForeignIdentity,
    Failed(anyhow::Error),
}

/// Offer one whole-span recovery of a debt occurrence.
///
/// The observation identity is the occurrence, not a segment inside it.
/// Every non-empty segment must sit wholly inside that span. A segment that
/// escapes, or a payload whose clock does not, is a refusal: nothing is admitted.
fn admit_debt_occurrence_recovery(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    occurrence: &OccurrenceIdentity,
    payload: &TailProviderPayload,
) -> bool {
    let exact_timing = payload.evidence.timing_quality
        == crate::stt::tail_provider::TailTimingQuality::ExactSampleRange
        || (cfg!(test)
            && payload.evidence.timing_quality
                == crate::stt::tail_provider::TailTimingQuality::Synthetic);
    let inside = |segment: &&TimedTailSegment| {
        segment.range.session == occurrence.session
            && segment.range.capture_epoch == occurrence.capture_epoch
            && segment.range.sample_start >= occurrence.sample_start
            && segment.range.sample_end <= occurrence.sample_end
    };
    let substantive: Vec<&TimedTailSegment> = payload
        .segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
        .collect();
    if !exact_timing
        || payload.validate().is_err()
        || substantive.is_empty()
        || substantive.iter().any(|segment| !inside(segment))
    {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "seal_coverage_text_recovery_refused".into(),
            message:
                "recovery evidence is not a source-mapped segment wholly inside the debt occurrence"
                    .into(),
        });
        return false;
    }
    let label = substantive
        .iter()
        .map(|segment| segment.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let _ = admit_ledger_label(
        state,
        ev_tx,
        LabelAdmission {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                payload.identity.request_id,
                1,
                occurrence.clone(),
            ),
            label: &label,
            energy: EnergyAdmission::RequireExistingQualification,
        },
    );
    let pending = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .text_recovery_pending(occurrence);
    if pending {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "seal_coverage_text_recovery_refused".into(),
            message: "recovery observation did not clear the debt occurrence".into(),
        });
    }
    !pending
}

fn range_overlaps_occurrence(range: &TailSampleRange, occurrence: &OccurrenceIdentity) -> bool {
    range.session == occurrence.session
        && range.capture_epoch == occurrence.capture_epoch
        && range.sample_start < occurrence.sample_end
        && occurrence.sample_start < range.sample_end
}

/// Compare committed occurrence coverage with the existing Silero speech
/// ledger (or the capture energy ladder when Silero produced no spans).
///
/// An occurrence that owes text recovery is requested on its own
/// `[sample_start, sample_end)`. The provider result is one whole-span
/// observation of that identity. A material uncovered range that no pending
/// debt occurrence owns keeps the gap path. Only mapped segments enter the
/// ledger. Unscoped text, uncertain timing and a segment that escapes the
/// requested span cannot manufacture occurrence evidence.
fn repair_terminal_seal_coverage(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    language: Option<&str>,
    execution: &LocalExecutionOwner,
) -> SealCoverageReceipt {
    repair_terminal_seal_coverage_with(
        state,
        ev_tx,
        language,
        execution,
        |request, pcm, control| InProcessTailProvider.transcribe_controlled(request, pcm, control),
    )
}

fn repair_terminal_seal_coverage_with<F>(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    language: Option<&str>,
    execution: &LocalExecutionOwner,
    transcribe: F,
) -> SealCoverageReceipt
where
    F: Fn(
            &TailProviderRequest,
            &[f32],
            &crate::stt::LocalExecutionControl,
        ) -> Result<TailProviderPayload>
        + Clone
        + Send
        + 'static,
{
    let threshold_samples =
        u64::from(state.sample_rate).saturating_mul(SEAL_COVERAGE_INCOMPLETE_MS) / 1_000;
    // One speech set for the whole terminal path. Repairing against a wider set
    // than the one the published receipt is measured against would send Whisper
    // after "gaps" that were never speech.
    let speech_evidence = coverage_speech_evidence(state);
    let initial = {
        let ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.assess_seal_coverage(
            &state.session_id,
            state.capture_epoch,
            &speech_evidence,
            threshold_samples,
        )
    };
    state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_seal_coverage(initial.clone());
    let mut debt = {
        let ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.pending_text_recoveries(&state.session_id, state.capture_epoch)
    };
    debt.sort_by_key(|occurrence| (occurrence.sample_start, occurrence.sample_end));
    if initial.status == SealCoverageStatus::Complete && debt.is_empty() {
        return initial;
    }
    // The live tail-patch drain already spent its own deadline. Recovery does
    // not lengthen that wait; it installs this phase's cap.
    execution.begin_text_recovery(SEAL_TEXT_RECOVERY_BUDGET);

    let mut ordinal = 0u64;
    let mut initial_published = false;
    let mut attempt = |state: &mut AppleSealState, range: TailSampleRange| -> StopRangeAttempt {
        let Some(window) = state.window_by_samples(range.sample_start, range.sample_end) else {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "seal_coverage_gap_pcm_unavailable".into(),
                message: format!(
                    "uncovered PCM {}..{} unavailable",
                    range.sample_start, range.sample_end
                ),
            });
            return StopRangeAttempt::MissingPcm;
        };
        let request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: u64::MAX - ordinal,
                range: range.clone(),
            },
            sample_rate: state.sample_rate,
            language: language.map(str::to_owned),
        };
        ordinal = ordinal.saturating_add(1);
        let job_request = request.clone();
        let transcribe = transcribe.clone();
        let result = execution
            .spawn(move |control| transcribe(&job_request, &window.samples, control))
            .and_then(|receiver| {
                // This is the existing blocking Apple worker, not a Tokio worker.
                // Native work remains retained even if the result channel fails.
                receiver.blocking_recv().map_err(anyhow::Error::from)?
            });
        let result = execution.check().and(result);
        if !initial_published {
            let _ = ev_tx.send(EngineEvent::SealCoverage {
                receipt: initial.clone(),
                comparison: None,
            });
            initial_published = true;
        }
        match result {
            Ok(payload) if payload.identity == request.identity => StopRangeAttempt::Ready(payload),
            Ok(_) => {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "seal_coverage_gap_identity_mismatch".into(),
                    message: "provider returned another PCM request".into(),
                });
                StopRangeAttempt::ForeignIdentity
            }
            Err(error) => StopRangeAttempt::Failed(error),
        }
    };
    let warn_failed = |ev_tx: &mpsc::UnboundedSender<EngineEvent>, error: anyhow::Error| {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "seal_coverage_gap_inference_failed".into(),
            message: error.to_string(),
        });
    };

    for occurrence in debt {
        if occurrence.sample_end <= occurrence.sample_start {
            continue;
        }
        let range = TailSampleRange {
            session: occurrence.session.clone(),
            capture_epoch: occurrence.capture_epoch,
            sample_start: occurrence.sample_start,
            sample_end: occurrence.sample_end,
        };
        match attempt(state, range) {
            StopRangeAttempt::Ready(payload) => {
                admit_debt_occurrence_recovery(state, ev_tx, &occurrence, &payload);
            }
            StopRangeAttempt::Failed(error) => warn_failed(ev_tx, error),
            StopRangeAttempt::MissingPcm | StopRangeAttempt::ForeignIdentity => {}
        }
    }

    let (after_debt, still_pending) = {
        let ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &speech_evidence,
                threshold_samples,
            ),
            ledger.pending_text_recoveries(&state.session_id, state.capture_epoch),
        )
    };
    // A range that still intersects pending debt was already requested as that
    // occurrence. Asking for its speech sub-range admits an overlap the
    // occurrence does not own.
    for range in after_debt.uncovered_speech_ranges {
        if range.sample_end.saturating_sub(range.sample_start) <= threshold_samples {
            continue;
        }
        if still_pending
            .iter()
            .any(|occurrence| range_overlaps_occurrence(&range, occurrence))
        {
            continue;
        }
        let requested = range.clone();
        match attempt(state, requested.clone()) {
            StopRangeAttempt::Ready(payload) => {
                admit_full_pass_gap_segments(
                    state,
                    ev_tx,
                    &payload,
                    std::slice::from_ref(&requested),
                    threshold_samples,
                    EnergyAdmission::QualifyFinalPassGap,
                );
            }
            StopRangeAttempt::Failed(error) => warn_failed(ev_tx, error),
            StopRangeAttempt::MissingPcm | StopRangeAttempt::ForeignIdentity => {}
        }
    }

    let final_receipt = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .assess_seal_coverage(
            &state.session_id,
            state.capture_epoch,
            &speech_evidence,
            threshold_samples,
        );
    state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_seal_coverage(final_receipt.clone());
    {
        let mut ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for range in &final_receipt.uncovered_speech_ranges {
            if range.sample_end.saturating_sub(range.sample_start) <= threshold_samples {
                continue;
            }
            ledger.note_unrecovered_speech(&OccurrenceIdentity::from(range));
        }
    }
    let _ = ev_tx.send(EngineEvent::SealCoverage {
        receipt: final_receipt.clone(),
        comparison: None,
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
    if ledger.text_recovery_pending(occurrence) {
        return false;
    }
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

    // The physical lane reconciles every timed candidate, including revised
    // earlier coordinates. The legacy cursor must not discard them first.
    if state.fusion_seal_armed {
        if segments.is_empty() {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "apple_final_without_pcm_timing".into(),
                message: "untimed Apple text cannot create an occurrence".into(),
            });
            return false;
        }
        seal_sliced_by_silero(state, ev_tx, &segments);
        return true;
    }
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
                .filter(|utterance| utterance.closed)
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
    consultation: Option<LiveConsultationCapture>,
    local_execution: Arc<LocalExecutionOwner>,
    sample_rate: u32,
    /// Device the recorder opened; selects the measured calibration profile.
    capture_device_name: Option<String>,
    language: Option<&'a str>,
    session_id: String,
    capture_epoch: u64,
    /// The capture arm's energy-ladder owner. The worker reads the same handle
    /// the async writer feeds; it never opens a ladder of its own.
    capture_energy: CaptureEnergyOwner,
    runtime_settings: Arc<RuntimeSettingsSnapshot>,
    acoustic_ledger: Arc<Mutex<AcousticLedger>>,
    settings_digest: String,
    /// Product "Hands-free silence". `Some` arms the engine lifecycle (speech
    /// epochs); `None` keeps one continuous SFSpeech stream for the whole take.
    utterance_silence_sec: Option<f32>,
    terminal_audio:
        Option<std_mpsc::Receiver<Result<super::live_audio_buffer::FinalizedPcmArchive, String>>>,
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
        mut consultation,
        local_execution,
        sample_rate,
        capture_device_name,
        language,
        session_id,
        capture_epoch,
        capture_energy,
        runtime_settings,
        acoustic_ledger,
        settings_digest,
        utterance_silence_sec,
        terminal_audio,
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
    state.bind_capture_energy(capture_energy);
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
        state.tick_refinements(&ev_tx, Instant::now());
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
        match pcm_rx.recv_timeout(LIVE_WORKER_QUANTUM) {
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
                    if let Some(owner) = consultation.as_mut()
                        && owner.observe(ingest, samples_seen).is_err()
                    {
                        owner.report_refusal(&ev_tx);
                    }
                    for evidence in &ingest.sideband {
                        let _ = ev_tx.send(EngineEvent::SidebandEvidence {
                            evidence: evidence.clone(),
                        });
                    }
                }
                if state.fusion_seal_armed {
                    if let Some(fusion) = state.fusion.as_ref() {
                        state.speech_progress.observe_speech(
                            &fusion.acoustic_speech_evidence(),
                            silero_ingest
                                .as_ref()
                                .is_some_and(|ingest| ingest.speech_live),
                            sample_rate,
                        );
                    }
                    seal_sliced_by_silero(&mut state, &ev_tx, &[]);
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
        if let Some(owner) = consultation.as_mut() {
            owner.tick(&state, &ev_tx);
        }
        state.emit_speech_integrity(&ev_tx);
    }

    if let Some(receiver) = terminal_audio {
        let archive = receiver
            .recv_timeout(Duration::from_secs(30))
            .map_err(|error| anyhow::anyhow!("terminal archive handoff failed: {error}"))
            .and_then(|receipt| receipt.map_err(anyhow::Error::msg))
            .and_then(|receipt| {
                receipt.load(
                    &state.session_id,
                    state.capture_epoch,
                    sample_rate,
                    samples_seen,
                )
            });
        match archive {
            Ok(pcm) => state.terminal_pcm = Some(pcm),
            Err(error) => {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "terminal_owned_pcm_unavailable".into(),
                    message: error.to_string(),
                });
                return Err(error);
            }
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

    if state.fusion_seal_armed {
        seal_sliced_by_silero(&mut state, &ev_tx, &[]);
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
    let stop_deadline = local_execution.begin_drain(TAIL_PATCH_CLOSURE_TIMEOUT);
    while state.tail_patch_awaiting_completion > 0 || !state.refinement_pending.is_empty() {
        let outstanding = state.tail_patch_awaiting_completion;
        if !state.stop_refinements_tick(&ev_tx, Instant::now(), stop_deadline) {
            tail_patch_timeout_residue = outstanding;
            break;
        }
        match tail_patch_done.recv_timeout(LIVE_WORKER_QUANTUM) {
            Ok(completion) => state.complete_whisper_window(&ev_tx, completion, audio_secs),
            Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
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
    drain_formatter_observers(&mut state, &ev_tx, &formatter_done)?;

    // Capture is over: no later Apple callback can revise a span and no further
    // Whisper window can arrive, so both double-close gates are satisfied by
    // definition. Seal the remainder here instead of leaving it to the residual
    // path — the machine's own span timestamps are the clock, because the audio
    // clock is frozen at EOF and can sit milliseconds behind them.
    state.seal_remaining_at_session_end(&ev_tx);
    repair_terminal_seal_coverage(&mut state, &ev_tx, language, &local_execution);
    drain_formatter_observers(&mut state, &ev_tx, &formatter_done)?;
    if let Some(owner) = consultation.as_mut() {
        owner.settle(&state, &ev_tx, samples_seen);
    }
    let seal_coverage = publish_terminal_coverage(&state, &ev_tx);
    state.emit_speech_integrity(&ev_tx);
    info!(
        session_id = %state.session_id,
        capture_epoch = state.capture_epoch,
        no_time_overlap_warnings = state.no_time_overlap_warnings,
        unmatched_word_identities = state.warned_unmatched_words.len(),
        retained_unmatched_words = state.unmatched_silero_words.len(),
        "apple_fusion_session_receipt"
    );
    // Any non-complete verdict takes the same non-success path. The warning
    // names which one it was; neither may reach the terminal seal below.
    if !seal_coverage.status.is_complete() {
        let (code, message) = match seal_coverage.status.unavailable_reason() {
            Some(gap) => (
                "terminal_seal_coverage_unavailable".to_string(),
                format!(
                    "reason={} producer={} availability={}",
                    gap.as_str(),
                    seal_coverage.speech_producer,
                    seal_coverage.availability,
                ),
            ),
            None => (
                "terminal_seal_coverage_incomplete".to_string(),
                format!(
                    "covered={}/{} max_uncovered={} threshold={}",
                    seal_coverage.covered_samples,
                    seal_coverage.speech_samples,
                    seal_coverage.max_uncovered_samples,
                    seal_coverage.incomplete_threshold_samples,
                ),
            ),
        };
        let _ = ev_tx.send(EngineEvent::Warning { code, message });
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
            conservation: state.session_conservation(),
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
        conservation: state.session_conservation(),
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
                state.observe_apple_progress(&text, &segments);
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
                // The preview paints where its words sit on the capture
                // counter. Segments give word grain. A segment-less partial
                // paints the occurrence still open since the last Apple
                // boundary, at utterance grain, and its receipt says so.
                let on_pcm = apple_segments_on_pcm_clock(state, &segments);
                let pin = match (
                    on_pcm.iter().map(|word| word.range.sample_start).min(),
                    on_pcm.iter().map(|word| word.range.sample_end).max(),
                ) {
                    (Some(sample_start), Some(sample_end)) => {
                        PreviewPin::from_segments(TailSampleRange {
                            session: state.session_id.clone(),
                            capture_epoch: state.capture_epoch,
                            sample_start,
                            sample_end,
                        })
                    }
                    _ => {
                        let captured_end = state.audio.session_sample_end();
                        let open_from = state.last_apple_segment_end.max(state.last_sealed_end);
                        let sample_start =
                            seconds_to_captured_sample(open_from, state.sample_rate, captured_end);
                        PreviewPin::open_occurrence(TailSampleRange {
                            session: state.session_id.clone(),
                            capture_epoch: state.capture_epoch,
                            sample_start,
                            sample_end: seconds_to_captured_sample(
                                audio_secs,
                                state.sample_rate,
                                captured_end,
                            )
                            .max(sample_start),
                        })
                    }
                };
                // Previews stay RAW: they are in-flight presentation, not
                // canvas, and correcting them would make the lexicon rewrite
                // flicker letter by letter while the phrase is still forming.
                state.open_partial = text.clone();
                state.open_partial_segments = segments;
                state.preview_rev = state.preview_rev.saturating_add(1);
                let _ = ev_tx.send(EngineEvent::Preview {
                    rev: state.preview_rev,
                    text,
                    pin,
                });
            }
            LiveStreamEvent::PhraseFinal { text, segments } => {
                state.observe_apple_progress(&text, &segments);
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

    fn consultation_owner() -> (
        LiveConsultationCapture,
        mpsc::Receiver<LiveConsultationRequest>,
        std_mpsc::Sender<LiveConsultationReturn>,
    ) {
        let (requests, rx) = mpsc::channel(CONSULTATION_QUEUE_CAP);
        let (tx, returns) = std_mpsc::channel();
        (
            LiveConsultationCapture {
                queue: ConsultationInputQueue::new("max-capture".into(), 1).unwrap(),
                requests,
                returns,
                last_assessed: None,
                assessment_pending: false,
                answers_pending: 0,
                speech_open: false,
                refused: false,
            },
            rx,
            tx,
        )
    }

    #[test]
    fn max_capture_uses_closed_edges_not_the_speech_live_bit_or_silence() {
        use super::super::silero_fusion::SileroIngest;
        let (mut owner, mut requests, _returns) = consultation_owner();
        owner.observe(&SileroIngest::default(), 16_000).unwrap();
        assert_eq!(
            owner.queue.pending_groups(),
            0,
            "silence cannot nominate speech"
        );
        owner
            .observe(
                &SileroIngest {
                    open: Some(1),
                    speech_live: true,
                    ..Default::default()
                },
                32_000,
            )
            .unwrap();
        assert!(owner.speech_open);
        assert_eq!(owner.queue.pending_groups(), 0);
        owner
            .observe(
                &SileroIngest {
                    closed: vec![1],
                    speech_live: true,
                    ..Default::default()
                },
                64_000,
            )
            .unwrap();
        assert!(
            !owner.speech_open,
            "the closing chunk still carries speech_live"
        );
        assert_eq!(owner.queue.pending_groups(), 1);
        owner.observe(&SileroIngest::default(), 72_000).unwrap();
        assert_eq!(
            owner.queue.pending_groups(),
            1,
            "quiet ticks keep one candidate"
        );
        owner
            .observe(
                &SileroIngest {
                    open: Some(2),
                    speech_live: true,
                    ..Default::default()
                },
                80_000,
            )
            .unwrap();
        assert_eq!(
            owner.queue.pending_groups(),
            0,
            "continuation invalidates assessment input"
        );
        owner
            .observe(
                &SileroIngest {
                    closed: vec![2],
                    speech_live: true,
                    ..Default::default()
                },
                96_000,
            )
            .unwrap();
        assert_eq!(owner.queue.pending_groups(), 1);
        assert!(
            requests.try_recv().is_err(),
            "edges alone cannot request semantic assessment"
        );
        assert!(!owner.assessment_pending);
    }

    #[test]
    fn max_stop_refuses_disconnected_return_owner_without_waiting_forever() {
        let (mut owner, _requests, returns) = consultation_owner();
        owner.assessment_pending = true;
        owner.answers_pending = 1;
        drop(returns);
        let state = state_for_session("max-capture");
        let (events, mut observed) = mpsc::unbounded_channel();
        owner.settle(&state, &events, 16_000);
        assert!(owner.refused);
        assert!(!owner.assessment_pending);
        assert_eq!(owner.answers_pending, 0);
        assert!(
            matches!(observed.try_recv().unwrap(), EngineEvent::Warning { code, .. }
            if code == "max_consultation_refused")
        );
        assert!(observed.try_recv().is_err());
        assert_eq!(
            owner.queue.pending_groups(),
            1,
            "lost transport cannot acknowledge source"
        );
    }

    #[test]
    fn max_async_errors_return_to_capture_without_fabricating_publication() {
        let (tx, rx) = std_mpsc::channel();
        let sink = crate::pipeline::sinks::CollectorEventSink::new();
        deliver_consultation_result(
            LiveConsultationResult::Assessed(Err(anyhow::anyhow!("assessment"))),
            &tx,
            &sink,
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            LiveConsultationReturn::Assessed(Err(_))
        ));
        deliver_consultation_result(
            LiveConsultationResult::Prepared(Err(anyhow::anyhow!("disk"))),
            &tx,
            &sink,
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            LiveConsultationReturn::Prepared(Err(_))
        ));
        deliver_consultation_result(
            LiveConsultationResult::Answer(Err(anyhow::anyhow!("execution"))),
            &tx,
            &sink,
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            LiveConsultationReturn::Published(Err(_))
        ));
        assert!(sink.events().is_empty());
    }

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

    /// W4-T14 regression reconstructed from the build-755 take. The Apple
    /// occurrence already owns the phrase before sample 837632. A whole-pass
    /// Whisper segment that repeats those words crosses that boundary, while
    /// the novel continuation is pinned wholly inside the uncovered range.
    /// Only the latter may become a document occurrence.
    #[test]
    fn real_take_gap_repair_rejects_straddled_repeat_and_keeps_novel_pcm_segment() {
        use crate::stt::tail_provider::{
            TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
            TailTimingQuality,
        };

        const CAPTURE_END: usize = 1_082_880;
        let session = "walkaround-755-gap-repro";
        let (tx, mut event_rx) = mpsc::unbounded_channel();
        let mut state = state_for_session(session);
        state.audio.push(&vec![0.25; CAPTURE_END]);

        let apple_occurrence = OccurrenceIdentity::new(session, 1, 694_272, 837_632);
        stage_pending_occurrence(
            &mut state,
            &tx,
            4,
            apple_occurrence,
            "Bo generalnie jest to dużo skuteczniejsze jeśli takie rzeczy są oczywiste w tym przypadku",
        );
        let gap = TailSampleRange {
            session: session.to_string(),
            capture_epoch: 1,
            sample_start: 837_632,
            sample_end: 1_082_880,
        };
        let payload = TailProviderPayload {
            identity: TailRequestIdentity {
                request_id: u64::MAX,
                range: TailSampleRange {
                    session: session.to_string(),
                    capture_epoch: 1,
                    sample_start: 694_272,
                    sample_end: 1_082_880,
                },
            },
            text: "Bo generalnie jest to dużo skuteczniejsze jeśli takie rzeczy są oczywiste w tym przypadku miały jakąś kanwę falsyfikacji"
                .to_string(),
            segments: vec![
                TimedTailSegment {
                    grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                    text: "takie rzeczy są oczywiste w tym przypadku".to_string(),
                    range: TailSampleRange {
                        session: session.to_string(),
                        capture_epoch: 1,
                        sample_start: 800_000,
                        sample_end: 880_000,
                    },
                },
                TimedTailSegment {
                    grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                    text: "miały jakąś kanwę falsyfikacji".to_string(),
                    range: TailSampleRange {
                        session: session.to_string(),
                        capture_epoch: 1,
                        sample_start: 900_000,
                        sample_end: 1_080_000,
                    },
                },
            ],
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.0),
            provider_id: TailProviderId::Fake,
            elapsed_ms: 1,
            evidence: TailProviderEvidence {
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                source: TailEvidenceSource::Whisper,
                revision: Some("walkaround-755".to_string()),
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: Some(-0.2),
            },
        };
        payload.validate().expect("repro payload must be valid");

        assert_eq!(
            admit_full_pass_gap_segments(
                &mut state,
                &tx,
                &payload,
                &[gap],
                4_000,
                EnergyAdmission::QualifyFinalPassGap,
            ),
            1
        );

        let rendered = state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .rendered_text();
        assert_eq!(rendered.matches("takie rzeczy").count(), 1, "{rendered}");
        assert!(rendered.ends_with("miały jakąś kanwę falsyfikacji"));

        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Warning { code, .. }
                if code == "seal_coverage_full_pass_segment_straddles_gap"
        )));
        assert!(events.iter().all(|event| !matches!(
            event,
            EngineEvent::LedgerMutation { label, .. }
                if label == "takie rzeczy są oczywiste w tym przypadku"
        )));
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

        // Keep a healthy receiver alive: Max must refuse this lane because of
        // its policy, not because the transport happens to be unavailable.
        let (max_tx, mut max_rx) = mpsc::channel(1);
        let max_formatter = live_formatter_lane_is_armed(
            CaptureTurnIntent::HandsFree,
            true,
            FormattingPolicy::Max,
            || true,
        )
        .then_some(max_tx);
        for (session, formatter) in {
            let (closed_tx, closed_rx) = mpsc::channel(1);
            drop(closed_rx);
            [
                ("formatter-disabled", None),
                ("formatter-closed", Some(closed_tx)),
                ("max-grouped-consultation", max_formatter),
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
        assert!(
            max_rx.try_recv().is_err(),
            "Max must not enqueue an occurrence job"
        );
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
                pin: PreviewPin::open_occurrence(TailSampleRange {
                    session: "drainage".into(),
                    capture_epoch: 1,
                    sample_start: 0,
                    sample_end: 0,
                }),
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
            EngineEvent::LedgerMutation { observation, receipt, .. }
                if observation.producer == LedgerObservationProducer::Whisper
                    && receipt.grants_mutation()
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,
                    EngineEvent::LedgerMutation { observation, receipt, .. }
                        if observation.producer == LedgerObservationProducer::Whisper
                            && !receipt.grants_mutation()
                ))
                .count(),
            2,
            "each member retains its no-label refusal receipt"
        );
        let ledger = state.acoustic_ledger.lock().expect("ledger");
        assert!(ledger.is_sealed(&first));
        assert!(ledger.is_sealed(&second));
        for occurrence in [&first, &second] {
            assert!(
                ledger
                    .frontier_of(occurrence)
                    .unwrap()
                    .open_producers()
                    .is_empty()
            );
        }
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
    /// A straddling segment intersects both members, so neither member is
    /// relabelled. The pin that sits wholly inside the second member stays
    /// visible and does not replace that member: one non-exclusive pin blocks
    /// the whole span.
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
        // inside the second member: the positive control owns 16 000..32 000.
        let identity = request.provider_request.identity.clone();
        let straddling = TimedTailSegment {
            grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
            text: "przez granice".to_string(),
            range: TailSampleRange {
                session: "straddle".to_string(),
                capture_epoch: 1,
                sample_start: 8_000,
                sample_end: 24_000,
            },
        };
        let pinned = TimedTailSegment {
            grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
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
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
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
                    observation,
                    label,
                    receipt,
                } if observation.producer == LedgerObservationProducer::Whisper
                    && receipt.grants_mutation() =>
                {
                    Some((observation.occurrence.clone(), label.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            whisper_labels,
            Vec::<(OccurrenceIdentity, String)>::new(),
            "a straddling pin blocks replacement of every member it intersects"
        );
        assert!(
            !whisper_labels
                .iter()
                .any(|(occurrence, _)| occurrence == &first),
            "a straddling candidate must not be poured into the first span"
        );

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                .count(),
            2
        );
        assert!(events.iter().any(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, receipt, .. }
                if observation.producer == LedgerObservationProducer::Whisper
                    && observation.occurrence == first && !receipt.grants_mutation()
        )));
        let ledger = state.acoustic_ledger.lock().expect("ledger");
        for occurrence in [&first, &second] {
            assert!(
                ledger
                    .frontier_of(occurrence)
                    .unwrap()
                    .open_producers()
                    .is_empty()
            );
        }
        assert!(ledger.is_sealed(&first));
        assert!(ledger.is_sealed(&second));
        assert_eq!(
            ledger.text_of(&first),
            Some("Iwo"),
            "the Apple floor survives a candidate that named no exact occurrence"
        );
        assert_eq!(
            ledger.text_of(&second),
            Some("Iwo"),
            "the wholly owned pin stays visible and does not replace the member"
        );
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
        assert_eq!(ledger.text_of(&second), Some("Iwo"));
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

        // True means accepted into scheduling, even when lane loss closes it.
        assert!(state.flush_layer1_coalesce(&tx));
        assert!(state.refinement_lane_lost);
        assert!(state.refinement_pending.is_empty());
        assert!(state.refinement_submitted.is_empty());
        assert!(state.layer1_coalesce.is_empty());
        let ledger = state.acoustic_ledger.lock().expect("ledger");
        for occurrence in [&first, &second] {
            assert!(ledger.is_sealed(occurrence));
            assert_eq!(ledger.text_of(occurrence), Some("Iwo"));
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
        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                .count(),
            2
        );
        let final_count = events
            .iter()
            .filter(|event| matches!(event, EngineEvent::UtteranceFinal { .. }))
            .count();
        assert_eq!(
            final_count, 2,
            "A and B each emit exactly one pending final"
        );
        assert!(!state.flush_layer1_coalesce(&tx));
        assert!(
            event_rx.try_recv().is_err(),
            "drained members cannot seal twice"
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
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
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
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
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
        emit_session_finalised(
            &sink,
            "test-session".to_string(),
            0,
            SessionConservationReceipt::default(),
        );
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
            SessionConservationReceipt::default(),
        );
        assert_eq!(receipt.applied, 1);
        assert_eq!(receipt.skipped, 1);
        assert_eq!(receipt.timed_out, 1);
        assert_eq!(receipt.abandoned, 0);
        assert!(receipt.is_reconciled());

        let worker_failed =
            tail_patch_receipt_after_stop(true, 2, None, SessionConservationReceipt::default());
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
            SessionConservationReceipt::default(),
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
            SessionConservationReceipt::default(),
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
        let mut lane = AppleTailPatchLane::new(
            TEST_SAMPLE_RATE,
            None,
            crate::stt::tail_provider::TailProviderId::InProcess,
        );
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
                admit_sample_start: 0,
                admit_sample_end: 16_000,
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
                admit_sample_start: 16_000,
                admit_sample_end: 32_000,
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

#[cfg(test)]
#[path = "seal_coverage_tests.rs"]
mod seal_coverage_tests;

#[cfg(test)]
#[path = "live_speech_edge_tests.rs"]
mod live_speech_edge_tests;

#[cfg(test)]
mod storm_tests {
    use super::*;
    const TEST_SAMPLE_RATE: u32 = 16_000;
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
    fn unmatched_words_warn_once_per_session() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new_for_session(TEST_SAMPLE_RATE, "storm-test".into(), 0);
        arm_fusion_slice_admission(&mut state);
        let words = vec![segment("leftover", 9.0, 9.1)];
        assert!(seal_sliced_by_silero(&mut state, &tx, &words));
        for _ in 0..100 {
            assert!(seal_sliced_by_silero(&mut state, &tx, &[]));
        }
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let warnings = events
            .iter()
            .filter(|event| {
                matches!(event,
            EngineEvent::Warning { code, .. } if code == "no_time_overlap")
            })
            .count();
        assert_eq!(warnings, 1, "unchanged leftovers must warn only once");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, EngineEvent::LedgerMutation { .. }))
        );

        // A repeated observation does not warn again; equal text on new PCM does.
        assert!(seal_sliced_by_silero(&mut state, &tx, &words));
        assert!(rx.try_recv().is_err());
        assert!(seal_sliced_by_silero(
            &mut state,
            &tx,
            &[segment("leftover", 9.2, 9.3)]
        ));
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == "no_time_overlap"))
                .count(),
            1
        );
    }

    #[test]
    fn no_time_overlap_retries_on_silero_extension_and_close() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new_for_session(TEST_SAMPLE_RATE, "storm-test".into(), 0);
        arm_fusion_slice_admission(&mut state);
        seal_sliced_by_silero(&mut state, &tx, &[segment("later", 9.0, 9.1)]);
        while rx.try_recv().is_ok() {}
        let fusion = state.fusion.as_mut().unwrap();
        let start = fusion
            .ledger()
            .utterances()
            .last()
            .unwrap()
            .range
            .sample_start;
        fusion.observe(Some((start, at(10.0))), false, at(10.0));
        seal_sliced_by_silero(&mut state, &tx, &[]);
        assert!(state.unmatched_silero_words.is_empty());
        assert!(!state.pending_silero_words.is_empty());
        state.fusion.as_mut().unwrap().observe(None, true, at(10.0));
        seal_sliced_by_silero(&mut state, &tx, &[]);
        assert!(state.pending_silero_words.is_empty());
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, EngineEvent::LedgerMutation { .. }))
        );
        assert!(!events.iter().any(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == "no_time_overlap")));
        assert_eq!(state.no_time_overlap_warnings, 1);
        assert_eq!(state.warned_unmatched_words.len(), 1);
    }

    #[test]
    fn silero_tick_with_retained_words_is_constant_cost() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new_for_session(TEST_SAMPLE_RATE, "storm-test".into(), 0);
        arm_fusion_slice_admission(&mut state);
        let words = (0..104)
            .map(|i| {
                let start = 9.0 + i as f32 * 0.01;
                segment("leftover", start, start + 0.005)
            })
            .collect::<Vec<_>>();
        assert!(seal_sliced_by_silero(&mut state, &tx, &words));
        while rx.try_recv().is_ok() {}
        let storage = state.unmatched_silero_words.as_ptr();
        let started = std::time::Instant::now();
        let mut retained_storage = true;
        for _ in 0..200 {
            assert!(seal_sliced_by_silero(&mut state, &tx, &[]));
            retained_storage &= storage == state.unmatched_silero_words.as_ptr();
        }
        let elapsed = started.elapsed();
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        eprintln!(
            "F3: 200 ticks, 104 words: {elapsed:?}, events={}",
            events.len()
        );
        assert!(
            events.is_empty(),
            "idle callbacks must not enqueue warnings or ledger mutations"
        );
        // Deterministic work bound, independent of host scheduling: no copying
        // the retained word vector on an unchanged Silero boundary snapshot.
        assert!(
            retained_storage,
            "idle callbacks must reuse retained word storage"
        );
        assert_eq!(state.unmatched_silero_words.len(), 104);
    }
}

/// rc-w1-live-ledger: acoustic speech coverage, occurrence-safe decode context,
/// and the diagnostic-versus-ledger receipt.
///
/// A separate `#[cfg(test)]` module on purpose. The file's main `mod tests` is
/// parked behind `#[cfg(any())]` (51 of this file's 65 `#[test]` functions),
/// so contracts written there are invisible to the compiler and to the suite.
/// These falsifiers are meant to run.
#[cfg(test)]
mod rc_w1_live_ledger_tests {
    use super::*;
    use crate::audio::capture_receipt::{AcousticAvailability, CAPTURE_ENERGY_PRODUCER};
    use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
    use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;

    const RATE: u32 = 16_000;
    /// `ACOUSTIC_SPEECH_PAD_SECS` (64 ms) at [`RATE`].
    const PAD: u64 = 1_024;
    /// `SEAL_COVERAGE_INCOMPLETE_MS` (250 ms) at [`RATE`].
    const THRESHOLD: u64 = 4_000;

    fn at(secs: f32) -> u64 {
        (secs * RATE as f32) as u64
    }

    fn push_capture(state: &mut AppleSealState, secs: f32) {
        let total = (secs * RATE as f32) as usize;
        let session = vec![0.25f32; total];
        for chunk in session.chunks(1024) {
            state.audio.push(chunk);
        }
    }

    fn crossing(kind: VadBoundaryKind, sample: u64) -> VadBoundaryEvidence {
        VadBoundaryEvidence {
            kind,
            sample,
            speech_probability: match kind {
                VadBoundaryKind::SpeechStart => 0.9,
                VadBoundaryKind::SpeechEnd => 0.1,
            },
        }
    }

    /// The producer decision: the narrowest observer that actually measured
    /// speech wins, padded ownership windows are never a candidate, and when no
    /// observer measured, the answer carries its own unavailability instead of
    /// an empty set that reads as silence.
    #[test]
    fn coverage_producer_picks_a_measuring_observer_and_never_ownership_windows() {
        // Ownership windows spanning the whole take, no crossings, no ladder.
        let mut state = AppleSealState::new_for_session(RATE, "rc-w3-producer".into(), 0);
        push_capture(&mut state, 10.0);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        ingress
            .ledger_mut()
            .open_or_extend(&state.session_id, 0, 0, at(10.0));
        ingress.ledger_mut().close_open(at(10.0));
        state.fusion = Some(ingress);
        assert_eq!(
            sum_range_samples(&fusion_utterance_ranges(&state)),
            at(10.0),
            "ownership keeps its padded window; this test is about who may use it"
        );

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            evidence.availability(),
            AcousticAvailability::NotObserved,
            "padded ownership is not an acoustic measurement, and nothing else measured"
        );
        assert!(evidence.ranges().is_empty());

        // The capture energy ladder measured the take: it becomes the observer.
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![0.25f32; at(1.0) as usize]);
        writer.push_samples(&vec![0.0f32; at(9.0) as usize]);
        let measured = coverage_speech_evidence(&state);
        assert_eq!(measured.producer(), CAPTURE_ENERGY_PRODUCER);
        assert!(measured.observed_speech());
        assert_eq!(measured.ranges().len(), 1);
        assert_eq!(measured.ranges()[0].sample_end, at(1.0));

        // Silero crossings are narrower still and outrank the ladder.
        let fusion = state.fusion.as_mut().unwrap();
        fusion.ingest(&vec![0.25f32; at(10.0) as usize], at(10.0));
        fusion.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(2.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(3.0)),
        ]);
        let acoustic = coverage_speech_evidence(&state);
        assert_eq!(acoustic.producer(), SILERO_BOUNDARIES_PRODUCER);
        assert_eq!(acoustic.ranges().len(), 1);
        assert_eq!(acoustic.ranges()[0].sample_start, at(2.0) - PAD);
    }

    /// Ownership padding cannot raise the acoustic speech figure, and a real
    /// measured gap still refuses. Both halves in one witness, because the
    /// failure this cut exists for was the padded set passing as coverage.
    #[test]
    fn ownership_padding_cannot_increase_measured_speech() {
        let mut state = AppleSealState::new_for_session(RATE, "rc-w3-padding".into(), 0);
        push_capture(&mut state, 10.0);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        ingress.ingest(&vec![0.25f32; at(10.0) as usize], at(10.0));
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
        ]);
        // The ownership window is the whole take; the crossing is one second.
        ingress
            .ledger_mut()
            .open_or_extend(&state.session_id, 0, 0, at(10.0));
        ingress.ledger_mut().close_open(at(10.0));
        state.fusion = Some(ingress);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
        assert_eq!(
            sum_range_samples(evidence.ranges()),
            at(1.0) + 2 * PAD,
            "the acoustic figure is the crossing plus its 64 ms margin, not the window"
        );
        assert!(
            sum_range_samples(evidence.ranges())
                < sum_range_samples(&fusion_utterance_ranges(&state)),
            "ownership padding may not raise measured speech"
        );

        // Nothing is committed, so the measured second is an uncovered gap.
        let receipt = {
            let ledger = state.acoustic_ledger.lock().unwrap();
            ledger.assess_seal_coverage(&state.session_id, 0, &evidence, THRESHOLD)
        };
        assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
        assert_eq!(receipt.max_uncovered_samples, at(1.0) + 2 * PAD);
    }

    /// The regression this cut exists for. A padded ownership window spanning a
    /// whole take must not be counted as speech; the crossings inside it are.
    ///
    /// Shape taken from an archived 50-minute take: one utterance window that
    /// stayed open for its whole length reported 99.5% of the recording as
    /// speech, against an offline Silero measurement of 454 s.
    #[test]
    fn coverage_speech_set_measures_crossings_not_the_padded_ownership_window() {
        let mut state = AppleSealState::new_for_session(RATE, "rc-w1-coverage".into(), 0);
        push_capture(&mut state, 10.0);

        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        // Two seconds of real speech inside a ten-second take.
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
            crossing(VadBoundaryKind::SpeechStart, at(8.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(9.0)),
        ]);
        // One padded ownership window covering the entire take, exactly as the
        // fusion ledger legitimately mints it.
        ingress.observe(Some((0, at(10.0))), true, at(10.0));
        ingress.note_observed_pcm(at(10.0), at(10.0));
        state.fusion = Some(ingress);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
        let speech = evidence.ranges();
        assert_eq!(speech.len(), 2, "two bursts, not one window");
        assert_eq!(speech[0].session, "rc-w1-coverage");

        let acoustic_samples = sum_range_samples(speech);
        let utterance_samples = sum_range_samples(&fusion_utterance_ranges(&state));
        assert_eq!(utterance_samples, at(10.0), "ownership keeps its window");
        // Two 1 s bursts, each padded 64 ms on both sides.
        assert_eq!(acoustic_samples, 2 * (at(1.0) + 2 * 1_024));
        assert!(
            acoustic_samples * 4 < utterance_samples,
            "the coverage set must shrink to the speech, not to the take"
        );
    }

    /// Without a fusion ingress at all, the capture energy ladder still answers.
    /// This lane may not remove the fallback that keeps a VAD-less take
    /// measurable — but the ladder must have actually measured to answer.
    #[test]
    fn coverage_falls_back_to_capture_energy_without_a_fusion_ingress() {
        let mut state = AppleSealState::new_for_session(RATE, "rc-w1-no-vad".into(), 0);
        push_capture(&mut state, 4.0);
        assert!(state.fusion.is_none());

        let unmeasured = coverage_speech_evidence(&state);
        assert_eq!(unmeasured.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            unmeasured.availability(),
            AcousticAvailability::NotObserved,
            "a ladder nobody fed cannot answer for a VAD-less take"
        );

        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![0.25f32; at(4.0) as usize]);
        let measured = coverage_speech_evidence(&state);
        assert_eq!(measured.producer(), CAPTURE_ENERGY_PRODUCER);
        assert!(measured.observed_speech());
    }

    /// An open crossing at stop is speech up to the capture cursor, and the
    /// coverage set may not claim audio past what was captured.
    #[test]
    fn coverage_speech_set_clamps_an_open_crossing_at_end_of_capture() {
        let mut state = AppleSealState::new_for_session(RATE, "rc-w1-eof".into(), 0);
        push_capture(&mut state, 3.0);

        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        ingress.observe_boundaries(&[crossing(VadBoundaryKind::SpeechStart, at(2.0))]);
        ingress.note_observed_pcm(at(3.0), at(3.0));
        state.fusion = Some(ingress);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
        let speech = evidence.ranges();
        assert_eq!(speech.len(), 1);
        assert_eq!(speech[0].sample_start, at(2.0) - 1_024);
        assert_eq!(
            speech[0].sample_end,
            at(3.0),
            "the right pad stops at the last captured sample"
        );
    }

    /// A word shorter than any proposed minimum-utterance figure is still
    /// speech the receipt must account for. Nothing on this lane may quietly
    /// drop it.
    #[test]
    fn coverage_speech_set_keeps_a_short_word() {
        let mut state = AppleSealState::new_for_session(RATE, "rc-w1-short".into(), 0);
        push_capture(&mut state, 5.0);

        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        // 80 ms of speech.
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(1.0) + 1_280),
        ]);
        ingress.note_observed_pcm(at(5.0), at(5.0));
        state.fusion = Some(ingress);

        let evidence = coverage_speech_evidence(&state);
        let speech = evidence.ranges();
        assert_eq!(speech.len(), 1, "a short word is not discarded");
        assert_eq!(speech[0].sample_start, at(1.0) - 1_024);
        assert_eq!(speech[0].sample_end, at(1.0) + 1_280 + 1_024);
    }

    /// The disputed labels were admitted, not lost. A `skipped`
    /// char-diff verdict is diagnostics; the payload it "skipped" is forwarded
    /// to occurrence admission all the same, and the receipt says both.
    #[test]
    fn a_legacy_skip_verdict_does_not_discard_the_forwarded_payload() {
        // The exact shape of the three disputed jobs: the char-diff clamped on
        // change ratio, and every one of those payloads still reached the
        // ledger and persisted to the Bus.
        let skipped = legacy_char_diff_receipt(
            &TailPatchOutcome::skipped(SkipReasonCode::ChangeRatio, "ratio 1.31 exceeds max 0.50"),
            true,
        );
        assert_eq!(skipped.verdict, "skipped");
        assert!(
            skipped.payload_forwarded,
            "a skipped verdict must not read as a discarded payload"
        );
        assert_eq!(skipped.admission_authority, LEDGER_ADMISSION_AUTHORITY);

        for (outcome, verdict) in [
            (TailPatchOutcome::NoChange, "no_change"),
            (
                TailPatchOutcome::skipped(SkipReasonCode::ChangeRatio, "ratio 1.90"),
                "skipped",
            ),
            (TailPatchOutcome::Patches(Vec::new()), "patches"),
        ] {
            let receipt = legacy_char_diff_receipt(&outcome, true);
            assert_eq!(receipt.verdict, verdict);
            assert!(
                receipt.payload_forwarded,
                "every completed job forwards its payload regardless of verdict"
            );
            assert_ne!(
                receipt.admission_authority, receipt.verdict,
                "the verdict is never the admission authority"
            );
        }

        // A failed job is the one case with no payload, and it says so.
        let failed = legacy_char_diff_receipt(&TailPatchOutcome::NoChange, false);
        assert!(!failed.payload_forwarded);
    }
}

/// W2 acoustic checkpoint: the terminal coverage entrypoints, the capture
/// energy fallback, and ledger admission — exercised through the production
/// functions rather than through a helper called twice.
///
/// These live in their own module because this file's main `mod tests` is
/// parked behind `#[cfg(any())]`; see the disposition table in the cut report.
/// Nothing here revives a retired production helper to satisfy an old test.
#[cfg(test)]
mod rc_w2_acoustic_tests {
    use super::*;
    use crate::audio::capture_receipt::{
        AcousticAvailability, CAPTURE_ENERGY_PRODUCER, CaptureEvidenceIdentity,
    };
    use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
    use crate::pipeline::acoustic_ledger::{AcousticEvidenceGap, RefuseReason};

    const RATE: u32 = 16_000;
    /// `ACOUSTIC_SPEECH_PAD_SECS` (64 ms) at [`RATE`].
    const PAD: u64 = 1_024;
    /// `SEAL_COVERAGE_INCOMPLETE_MS` (250 ms) at [`RATE`].
    const THRESHOLD: u64 = 4_000;

    // capture_receipt owns a per-test-thread clock for every energy consumer.
    // No module-local mutex can protect a process-global reset/feed/read sequence.

    fn at(secs: f32) -> u64 {
        (secs * RATE as f32) as u64
    }

    fn state_for(session: &str, capture_secs: f32) -> AppleSealState {
        let mut state = AppleSealState::new_for_session(RATE, session.into(), 0);
        state.energy_calibration = Some(EnergyCalibration::new("synthetic", 1.0, 1));
        if capture_secs > 0.0 {
            state
                .audio
                .push(&vec![0.25f32; (capture_secs * RATE as f32) as usize]);
        }
        state
    }

    fn crossing(kind: VadBoundaryKind, sample: u64) -> VadBoundaryEvidence {
        VadBoundaryEvidence {
            kind,
            sample,
            speech_probability: match kind {
                VadBoundaryKind::SpeechStart => 0.9,
                VadBoundaryKind::SpeechEnd => 0.1,
            },
        }
    }

    /// One second of speech inside a ten-second take, plus the padded ownership
    /// window the fusion ledger legitimately mints across the whole take.
    ///
    /// The two sets disagree by an order of magnitude on purpose: every
    /// assertion below is only meaningful because a regression to the padded
    /// set would change the answer.
    fn one_burst_in_a_ten_second_take(session: &str) -> AppleSealState {
        let mut state = state_for(session, 10.0);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        // The same ten seconds `state_for` pushed into the audio buffer reached
        // the VAD. Synthetic crossings still stand in for the model read.
        ingress.note_observed_pcm(at(10.0), at(10.0));
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
        ]);
        ingress.observe(Some((0, at(10.0))), true, at(10.0));
        state.fusion = Some(ingress);
        state
    }

    fn two_bursts(session: &str) -> AppleSealState {
        let mut state = state_for(session, 10.0);
        let mut ingress = SileroIngress::new(RATE, session, 0);
        ingress.note_observed_pcm(at(10.0), at(10.0));
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
            crossing(VadBoundaryKind::SpeechStart, at(4.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(5.0)),
        ]);
        ingress.observe(Some((0, at(10.0))), true, at(10.0));
        state.fusion = Some(ingress);
        state
    }

    fn gap_payload(request: &TailProviderRequest) -> TailProviderPayload {
        use crate::stt::tail_provider::{
            TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
            TailTimingQuality,
        };
        TailProviderPayload {
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
            evidence: TailProviderEvidence {
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                source: TailEvidenceSource::Whisper,
                revision: None,
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: None,
            },
        }
    }

    /// A provider segment that starts before the uncovered gap is not speech
    /// this gap owns. The stop path must leave that gap as one named refusal
    /// the conservation receipt counts, and must not commit the straddling text.
    #[test]
    fn straddling_gap_segment_stays_a_named_unrecovered_refusal() {
        let mut state = two_bursts("straddle-gap");
        let expected = coverage_speech_evidence(&state).ranges().to_vec();
        assert!(!expected.is_empty());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execution = LocalExecutionOwner::default();
        let receipt = repair_terminal_seal_coverage_with(
            &mut state,
            &tx,
            None,
            &execution,
            move |request, pcm, control| {
                control.check()?;
                request.validate_pcm(pcm)?;
                let mut payload = gap_payload(request);
                let start = payload.segments[0].range.sample_start;
                payload.segments[0].range.sample_start = start.saturating_sub(500);
                payload.segments[0].text = "Straddle".into();
                payload.text = "Straddle".into();
                Ok(payload)
            },
        );
        assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
        assert!(
            !state
                .acoustic_ledger
                .lock()
                .unwrap()
                .rendered_text()
                .contains("Straddle"),
            "a segment that starts before the gap must not become committed text"
        );
        let _ = warning_codes(&mut rx);
        let conservation = state.session_conservation();
        assert_eq!(
            conservation
                .observations_refused_by_reason
                .get("unrecovered_speech")
                .copied(),
            Some(expected.len() as u64),
            "each material gap the stop path could not recover needs one named refusal: {conservation:?}"
        );
        assert_eq!(conservation.residue(), 0, "{conservation:?}");
    }

    /// Five committed debt occurrences, each wider than the Silero speech
    /// inside it, plus one speech burst that has no occurrence. The shape is
    /// take 9608b50e: the label is committed, the speech inside it is still
    /// debt, and a provider that answers with a segment wholly inside the
    /// requested range must recover the occurrence itself.
    fn commit_debt_occurrence(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        sample_start: u64,
        sample_end: u64,
        label: &str,
    ) -> OccurrenceIdentity {
        let occurrence = OccurrenceIdentity::new(
            state.session_id.clone(),
            state.capture_epoch,
            sample_start,
            sample_end,
        );
        assert!(
            qualify_owned_occurrence(state, &occurrence),
            "the fixture occurrence must qualify before it can owe recovery"
        );
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            ledger.schedule_frontier(
                occurrence.clone(),
                [
                    LedgerObservationProducer::Apple,
                    LedgerObservationProducer::Lexicon,
                ],
            );
            assert!(ledger.require_text_recovery(&occurrence));
        }
        let apple = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    sample_start,
                    0,
                    occurrence.clone(),
                ),
                label,
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );
        assert!(
            apple.is_some_and(|receipt| receipt.grants_mutation()),
            "Apple must commit the provisional label"
        );
        let lexicon = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    sample_start,
                    0,
                    occurrence.clone(),
                ),
                label,
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );
        assert!(lexicon.is_some(), "Lexicon closes the scheduled frontier");
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(ledger.text_recovery_pending(&occurrence));
        assert!(!ledger.is_sealed(&occurrence));
        occurrence
    }

    fn five_debt_occurrences(session: &str) -> (AppleSealState, Vec<OccurrenceIdentity>) {
        let stride = at(3.5);
        let gap_start = 5 * stride + at(0.5);
        let gap_end = gap_start + at(1.0);
        let capture_end = gap_end + at(0.5);
        let mut state = state_for(session, 0.0);
        state.audio.push(&vec![0.25f32; capture_end as usize]);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), state.capture_epoch);
        ingress.note_observed_pcm(capture_end, capture_end);
        let mut boundaries = Vec::new();
        for index in 0..5 {
            let speech_start = index * stride + at(0.6);
            let speech_end = index * stride + at(1.2);
            boundaries.push(crossing(VadBoundaryKind::SpeechStart, speech_start));
            boundaries.push(crossing(VadBoundaryKind::SpeechEnd, speech_end));
        }
        boundaries.push(crossing(VadBoundaryKind::SpeechStart, gap_start));
        boundaries.push(crossing(VadBoundaryKind::SpeechEnd, gap_end));
        ingress.observe_boundaries(&boundaries);
        state.fusion = Some(ingress);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut occurrences = Vec::new();
        for index in 0..5 {
            let start = index * stride;
            occurrences.push(commit_debt_occurrence(
                &mut state,
                &tx,
                start,
                start + at(2.0),
                &format!("apple {index}"),
            ));
        }
        (state, occurrences)
    }

    fn event_trace(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> String {
        let mut lines = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                EngineEvent::LedgerMutation {
                    observation,
                    receipt,
                    label,
                } => lines.push(format!(
                    "mutation {} {}..{} {} {label}",
                    observation.producer.as_str(),
                    observation.occurrence.sample_start,
                    observation.occurrence.sample_end,
                    receipt.as_str()
                )),
                EngineEvent::Warning { code, message } => {
                    lines.push(format!("warn {code}: {message}"))
                }
                _ => {}
            }
        }
        lines.join("\n")
    }

    #[test]
    fn debt_stop_path_recovers_each_occurrence_span_not_its_speech_subrange() {
        let (mut state, occurrences) = five_debt_occurrences("debt-span");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&calls);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execution = LocalExecutionOwner::default();
        let receipt = repair_terminal_seal_coverage_with(
            &mut state,
            &tx,
            Some("pl"),
            &execution,
            move |request, pcm, control| {
                control.check()?;
                request.validate_pcm(pcm)?;
                observed
                    .lock()
                    .unwrap()
                    .push(request.identity.range.clone());
                Ok(gap_payload(request))
            },
        );
        let calls = calls.lock().unwrap();
        let trace = event_trace(&mut rx);
        assert!(
            calls.len() >= occurrences.len(),
            "stop path made no occurrence request: {calls:?}\n{trace}"
        );
        for (call, occurrence) in calls.iter().take(occurrences.len()).zip(&occurrences) {
            assert_eq!(
                (call.sample_start, call.sample_end),
                (occurrence.sample_start, occurrence.sample_end),
                "debt recovery requested {call:?}, not the occurrence span\ncalls={calls:?}\n{trace}"
            );
        }
        let ledger = state.acoustic_ledger.lock().unwrap();
        let pending = ledger.pending_text_recoveries(&state.session_id, state.capture_epoch);
        assert!(
            pending.is_empty(),
            "debt stayed pending after wholly contained segments\ncalls={calls:?}\npending={pending:?}\n{trace}"
        );
        assert_eq!(receipt.status, SealCoverageStatus::Complete, "{receipt:?}");
        assert!(receipt.covered_samples > 0, "{receipt:?}");
        for occurrence in &occurrences {
            assert_eq!(ledger.text_of(occurrence), Some("Iwo"));
            assert!(!ledger.text_recovery_pending(occurrence));
        }
        drop(ledger);
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger
                    .seal_terminal(&state.session_id, state.capture_epoch)
                    .is_ok(),
                "cleared debt and complete coverage must allow the terminal seal"
            );
        }
        assert_eq!(state.session_conservation().residue(), 0);
    }

    /// The live tail-patch drain and text recovery share one execution owner.
    /// An expired live deadline must not cancel the recovery phase.
    #[test]
    fn debt_recovery_runs_after_the_live_drain_deadline_expired() {
        let (mut state, occurrences) = five_debt_occurrences("debt-after-drain");
        let execution = LocalExecutionOwner::default();
        execution.begin_drain(Duration::ZERO);
        let (tx, _rx) = mpsc::unbounded_channel();
        let receipt = repair_terminal_seal_coverage_with(
            &mut state,
            &tx,
            None,
            &execution,
            |request, pcm, control| {
                control.check()?;
                request.validate_pcm(pcm)?;
                Ok(gap_payload(request))
            },
        );
        assert_eq!(receipt.status, SealCoverageStatus::Complete, "{receipt:?}");
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger
                .pending_text_recoveries(&state.session_id, state.capture_epoch)
                .is_empty(),
            "an expired live drain cancelled recovery of {} occurrences",
            occurrences.len()
        );
    }

    #[test]
    fn recovery_segment_that_escapes_the_occurrence_stays_a_refusal() {
        let (mut state, occurrences) = five_debt_occurrences("debt-escape");
        let occurrence = occurrences[0].clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&calls);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execution = LocalExecutionOwner::default();
        let _receipt = repair_terminal_seal_coverage_with(
            &mut state,
            &tx,
            None,
            &execution,
            move |request, pcm, control| {
                control.check()?;
                request.validate_pcm(pcm)?;
                observed
                    .lock()
                    .unwrap()
                    .push(request.identity.range.clone());
                let mut payload = gap_payload(request);
                let end = payload.segments[0].range.sample_end;
                payload.segments[0].range.sample_end = end.saturating_add(500);
                payload.segments[0].text = "Escaped".into();
                payload.text = "Escaped".into();
                Ok(payload)
            },
        );
        let calls = calls.lock().unwrap();
        let trace = event_trace(&mut rx);
        assert!(
            calls.iter().any(|call| {
                call.sample_start == occurrence.sample_start
                    && call.sample_end == occurrence.sample_end
            }),
            "escape was not judged against the occurrence span\ncalls={calls:?}\n{trace}"
        );
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger.text_recovery_pending(&occurrence),
                "an escaping segment cleared debt\n{trace}"
            );
            assert_ne!(ledger.text_of(&occurrence), Some("Escaped"));
            assert!(
                !ledger.rendered_text().contains("Escaped"),
                "escaped text entered the document"
            );
            assert_eq!(
                ledger.seal_terminal(&state.session_id, state.capture_epoch),
                Err(SealRefusal::TextRecoveryPending)
            );
        }
        assert_eq!(
            state
                .session_conservation()
                .observations_refused_by_reason
                .get("unrecovered_speech")
                .copied(),
            Some(calls.len() as u64),
            "each unrecovered range is one named refusal\ncalls={calls:?}\n{trace}"
        );
    }

    #[test]
    fn owned_terminal_repair_still_admits_both_exact_gaps() {
        let mut state = two_bursts("owned-repair");
        let ranges = coverage_speech_evidence(&state).ranges().to_vec();
        assert_eq!(ranges.len(), 2);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&calls);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execution = LocalExecutionOwner::default();
        let receipt = repair_terminal_seal_coverage_with(
            &mut state,
            &tx,
            Some("pl"),
            &execution,
            move |request, pcm, control| {
                control.check()?;
                request.validate_pcm(pcm)?;
                observed.lock().unwrap().push(request.identity.clone());
                Ok(gap_payload(request))
            },
        );
        assert_eq!(receipt.status, SealCoverageStatus::Complete);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for (call, range) in calls.iter().zip(&ranges) {
            assert_eq!(&call.range, range);
            assert_eq!(
                state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .text_of(&OccurrenceIdentity::from(range)),
                Some("Iwo")
            );
        }
        assert_ne!(calls[0].request_id, calls[1].request_id);
        assert!(
            !warning_codes(&mut rx)
                .iter()
                .any(|code| code == "seal_coverage_gap_inference_failed")
        );
    }

    #[test]
    fn multigap_expiry_uses_one_budget_and_cannot_publish_late_native_success() {
        let mut state = two_bursts("expired-repair");
        let expected_ranges = coverage_speech_evidence(&state).ranges().to_vec();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execution = LocalExecutionOwner::default();
        let receipt = repair_terminal_seal_coverage_with(
            &mut state,
            &tx,
            None,
            &execution,
            move |request, pcm, control| {
                request.validate_pcm(pcm)?;
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // A native decode returned after the shared budget expired.
                control.limit_until(Instant::now());
                Ok(gap_payload(request))
            },
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
        assert_eq!(receipt.uncovered_speech_ranges, expected_ranges);
        assert_eq!(receipt.covered_samples, 0);
        assert_eq!(receipt.session_id, "expired-repair");
        assert_eq!(receipt.capture_epoch, 0);
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(
                event,
                EngineEvent::LedgerMutation { .. } | EngineEvent::LedgerSeal { .. }
            ));
        }
    }

    #[test]
    fn terminal_failure_and_foreign_identity_preserve_uncovered_pcm() {
        for foreign in [false, true] {
            let mut state = two_bursts("original-repair");
            let expected_ranges = coverage_speech_evidence(&state).ranges().to_vec();
            let (tx, mut rx) = mpsc::unbounded_channel();
            let execution = LocalExecutionOwner::default();
            let receipt = repair_terminal_seal_coverage_with(
                &mut state,
                &tx,
                None,
                &execution,
                move |request, _, _| {
                    if !foreign {
                        anyhow::bail!("injected local failure");
                    }
                    let mut payload = gap_payload(request);
                    payload.identity.range.session = "successor".into();
                    Ok(payload)
                },
            );
            assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
            assert_eq!(receipt.uncovered_speech_ranges, expected_ranges);
            assert_eq!(receipt.covered_samples, 0);
            let codes = warning_codes(&mut rx);
            assert!(codes.iter().any(|code| code
                == if foreign {
                    "seal_coverage_gap_identity_mismatch"
                } else {
                    "seal_coverage_gap_inference_failed"
                }));
        }
    }

    /// Commit the exact acoustic range as an occurrence, through the same
    /// admission path the terminal gap repair uses.
    fn commit_the_burst(
        state: &mut AppleSealState,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    ) -> OccurrenceIdentity {
        let occurrence = OccurrenceIdentity::new(
            state.session_id.clone(),
            state.capture_epoch,
            at(1.0) - PAD,
            at(2.0) + PAD,
        );
        let receipt = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Whisper,
                    1,
                    0,
                    occurrence.clone(),
                ),
                label: "Iwo",
                energy: EnergyAdmission::QualifyFinalPassGap,
            },
        );
        assert!(
            receipt.is_some_and(|receipt| receipt.grants_mutation()),
            "the fixture must actually commit an occurrence, or coverage proves nothing"
        );
        occurrence
    }

    fn warning_codes(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> Vec<String> {
        let mut codes = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::Warning { code, .. } = event {
                codes.push(code);
            }
        }
        codes
    }

    fn coverage_receipts(
        rx: &mut mpsc::UnboundedReceiver<EngineEvent>,
    ) -> Vec<SealCoverageReceipt> {
        let mut receipts = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::SealCoverage { receipt, .. } = event {
                receipts.push(receipt);
            }
        }
        receipts
    }

    /// The actual repair entrypoint, on a take whose committed occurrence covers
    /// the acoustic speech exactly.
    ///
    /// Negative control, and the reason this test is not a tautology: the same
    /// ledger, asked about the padded ownership windows instead, answers
    /// `Incomplete` with a 127 000-sample hole. If `repair_terminal_seal_coverage`
    /// ever drifts back to that set, `Complete` below stops holding.
    #[test]
    fn repair_measures_the_acoustic_set_and_leaves_a_covered_take_alone() {
        let mut state = one_burst_in_a_ten_second_take("rc-w2-repair-covered");
        let (tx, mut rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);
        let _ = warning_codes(&mut rx);

        // What the padded ownership set *would* answer if it were still allowed
        // to be the speech measurement. It is not — production reads
        // `coverage_speech_evidence` — so this is minted explicitly here as the
        // counterfactual the assertion below depends on.
        let padded_answer = {
            let ledger = state.acoustic_ledger.lock().unwrap();
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &AcousticSpeechEvidence::measured(
                    CaptureEvidenceIdentity::new(&state.session_id, state.capture_epoch),
                    "fusion_utterances_counterfactual",
                    AcousticAvailability::Observed {
                        observed_samples: state.audio.session_sample_end(),
                    },
                    fusion_utterance_ranges(&state),
                ),
                THRESHOLD,
            )
        };
        assert_eq!(
            padded_answer.status,
            SealCoverageStatus::Incomplete,
            "the padded ownership set must disagree, or this test proves nothing"
        );
        assert_eq!(padded_answer.speech_samples, at(10.0));

        let receipt = repair_terminal_seal_coverage(
            &mut state,
            &tx,
            Some("pl"),
            &LocalExecutionOwner::default(),
        );

        assert_eq!(receipt.status, SealCoverageStatus::Complete);
        assert!(receipt.uncovered_speech_ranges.is_empty());
        assert_eq!(
            receipt.speech_samples,
            at(1.0) + 2 * PAD,
            "coverage is measured against the crossings, not the ten-second window"
        );
        assert_eq!(receipt.incomplete_threshold_samples, THRESHOLD);
        assert!(
            warning_codes(&mut rx)
                .iter()
                .all(|code| !code.starts_with("seal_coverage_gap")),
            "a covered take must not send Whisper after gaps that were never speech"
        );
    }

    /// The actual publication entrypoint reports the acoustic set, records it on
    /// the ledger, and emits it once. The padded sum stays a diagnostic.
    #[test]
    fn publish_reports_the_acoustic_set_and_records_it_on_the_ledger() {
        let mut state = one_burst_in_a_ten_second_take("rc-w2-publish");
        let (tx, mut rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);
        let _ = coverage_receipts(&mut rx);

        let receipt = publish_terminal_coverage(&state, &tx);

        assert_eq!(receipt.speech_samples, at(1.0) + 2 * PAD);
        assert_eq!(receipt.status, SealCoverageStatus::Complete);
        assert_eq!(
            sum_range_samples(&fusion_utterance_ranges(&state)),
            at(10.0),
            "ownership keeps its padded window; only the coverage question narrowed"
        );
        assert_eq!(
            coverage_receipts(&mut rx),
            vec![receipt.clone()],
            "one publication, carrying exactly the receipt that was returned"
        );
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .latest_seal_coverage()
                .cloned(),
            Some(receipt),
            "terminal admission reads this receipt; it must be the recorded one"
        );
    }

    /// Repair and publication are two different production functions. They must
    /// answer with the same speech set on the same state, or repair chases gaps
    /// the published receipt never claimed.
    ///
    /// This replaces the W1 test that called one helper twice: both named
    /// entrypoints are invoked here, and each one's own receipt is compared.
    #[test]
    fn repair_and_publish_answer_with_one_speech_set() {
        let mut state = one_burst_in_a_ten_second_take("rc-w2-one-set");
        let (tx, mut rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);
        let _ = coverage_receipts(&mut rx);

        let published_before = publish_terminal_coverage(&state, &tx);
        let repaired = repair_terminal_seal_coverage(
            &mut state,
            &tx,
            Some("pl"),
            &LocalExecutionOwner::default(),
        );
        let published_after = publish_terminal_coverage(&state, &tx);

        for other in [&repaired, &published_after] {
            assert_eq!(other.speech_samples, published_before.speech_samples);
            assert_eq!(other.covered_samples, published_before.covered_samples);
            assert_eq!(
                other.incomplete_threshold_samples,
                published_before.incomplete_threshold_samples
            );
            assert_eq!(other.status, published_before.status);
            assert_eq!(
                other.uncovered_speech_ranges,
                published_before.uncovered_speech_ranges
            );
        }
    }

    /// Uncovered speech whose PCM cannot be resolved is reported, not decoded
    /// and not quietly dropped. No witness is manufactured for audio the process
    /// no longer holds.
    #[test]
    fn repair_reports_unresolvable_gap_pcm_instead_of_inventing_a_witness() {
        // Capture energy measured three seconds of speech; the live buffer holds
        // none of it, which is exactly the retention-loss case.
        let mut state = state_for("rc-w2-unresolvable", 0.0);
        assert!(state.fusion.is_none());
        let mut accumulator = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        accumulator.push_samples(&vec![0.25f32; at(3.0) as usize]);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let receipt = repair_terminal_seal_coverage(
            &mut state,
            &tx,
            Some("pl"),
            &LocalExecutionOwner::default(),
        );

        assert_eq!(receipt.status, SealCoverageStatus::Incomplete);
        assert_eq!(receipt.speech_samples, at(3.0));
        assert_eq!(receipt.covered_samples, 0);
        assert!(
            warning_codes(&mut rx).contains(&"seal_coverage_gap_pcm_unavailable".to_string()),
            "an unresolvable gap is a stated refusal, not a silent skip"
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert!(
            ledger.rendered_text().is_empty(),
            "no text may enter the ledger from a range whose PCM was never read"
        );
        assert_eq!(ledger.qualified_occurrences().count(), 0);
    }

    /// The capture energy fallback measures the hops the capture path actually
    /// recorded — not a flag, and not the whole take.
    #[test]
    fn capture_energy_fallback_measures_recorded_hops() {
        let state = state_for("rc-w2-energy", 2.0);
        assert!(state.fusion.is_none());
        let mut accumulator = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        accumulator.push_samples(&vec![0.25f32; at(1.0) as usize]);
        accumulator.push_samples(&vec![0.0f32; at(1.0) as usize]);

        let evidence = coverage_speech_evidence(&state);

        assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            evidence.availability(),
            AcousticAvailability::Observed {
                observed_samples: at(2.0)
            }
        );
        let speech = evidence.ranges();
        assert_eq!(speech.len(), 1, "the silent second is not speech");
        assert_eq!(speech[0].sample_start, 0);
        assert_eq!(speech[0].sample_end, at(1.0));
        assert_eq!(speech[0].session, "rc-w2-energy");
        assert_eq!(speech[0].capture_epoch, state.capture_epoch);
    }

    /// The gap this cut closes. Captured zeros and no samples at all both
    /// produce an empty speech set, and they must no longer produce the same
    /// finality: the first is a measurement whose answer is silence, the second
    /// is no measurement.
    #[test]
    fn measured_silence_and_absent_evidence_reach_different_finality() {
        let silent_state = state_for("rc-w2-silent", 1.0);
        let mut accumulator = CaptureLevelAccumulator::bound_to(&silent_state.capture_energy);
        accumulator.push_samples(&vec![0.0f32; at(1.0) as usize]);
        let silent_evidence = coverage_speech_evidence(&silent_state);

        let absent_state = state_for("rc-w2-absent", 1.0);
        let absent_evidence = coverage_speech_evidence(&absent_state);

        assert!(
            silent_evidence.ranges().is_empty(),
            "silence is never speech"
        );
        assert!(
            absent_evidence.ranges().is_empty(),
            "absence is never speech"
        );
        assert_eq!(silent_evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(absent_evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            silent_evidence.availability(),
            AcousticAvailability::Observed {
                observed_samples: at(1.0)
            }
        );
        assert_eq!(
            absent_evidence.availability(),
            AcousticAvailability::NotObserved
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let silent = publish_terminal_coverage(&silent_state, &tx);
        let absent = publish_terminal_coverage(&absent_state, &tx);
        assert_eq!(silent.speech_samples, 0);
        assert_eq!(absent.speech_samples, 0);
        assert_eq!(
            silent.status,
            SealCoverageStatus::Complete,
            "a ladder that ran and heard nothing has covered everything there was"
        );
        assert_eq!(
            absent.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::NotObserved),
            "a ladder that never ran cannot certify the take"
        );
        assert_eq!(silent.coverage_ratio(), Some(1.0));
        assert_eq!(
            absent.coverage_ratio(),
            None,
            "absence of measurement has no ratio"
        );
        assert_eq!(silent.observed_samples, Some(at(1.0)));
        assert_eq!(absent.observed_samples, None);

        // And the finality consumers agree: one may seal, the other may not.
        assert!(
            silent_state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_terminal("rc-w2-silent", 0)
                .is_err(),
            "an empty ledger still has no occurrence to seal"
        );
        assert_eq!(
            absent_state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_terminal("rc-w2-absent", 0),
            Err(SealRefusal::CoverageIncomplete),
            "unavailable measurement refuses before the ledger even looks for occurrences"
        );
    }

    /// rc-w3-acoustic-validity: three capture qualities, three honest outcomes,
    /// all through `publish_terminal_coverage` and the ledger's own finality.
    ///
    /// Valid silence still succeeds — that is the outcome the validity repair
    /// must not cost. An all-invalid capture and a mixed valid/invalid capture
    /// both refuse, and both name invalid measurement rather than a gap or an
    /// absent observer.
    #[test]
    fn valid_silence_all_invalid_and_mixed_capture_reach_distinct_outcomes() {
        let silent_state = state_for("rc-w3-quality-silent", 1.0);
        let mut silent_writer = CaptureLevelAccumulator::bound_to(&silent_state.capture_energy);
        silent_writer.push_samples(&vec![0.0f32; at(1.0) as usize]);

        let invalid_state = state_for("rc-w3-quality-invalid", 1.0);
        let mut invalid_writer = CaptureLevelAccumulator::bound_to(&invalid_state.capture_energy);
        invalid_writer.push_samples(&vec![f32::NAN; at(1.0) as usize]);

        let mixed_state = state_for("rc-w3-quality-mixed", 2.0);
        let mut mixed_writer = CaptureLevelAccumulator::bound_to(&mixed_state.capture_energy);
        mixed_writer.push_samples(&vec![0.0f32; at(1.0) as usize]);
        mixed_writer.push_samples(&vec![f32::INFINITY; at(1.0) as usize]);

        let (tx, _rx) = mpsc::unbounded_channel();
        let silent = publish_terminal_coverage(&silent_state, &tx);
        let invalid = publish_terminal_coverage(&invalid_state, &tx);
        let mixed = publish_terminal_coverage(&mixed_state, &tx);

        assert_eq!(
            silent.status,
            SealCoverageStatus::Complete,
            "a ladder that ran over finite silence covered everything there was"
        );
        assert_eq!(silent.coverage_ratio(), Some(1.0));
        assert_eq!(silent.observed_samples, Some(at(1.0)));

        for (label, receipt) in [("all invalid", &invalid), ("mixed", &mixed)] {
            assert_eq!(
                receipt.status,
                SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement),
                "{label} capture measured nothing it can stand behind"
            );
            assert_eq!(receipt.coverage_ratio(), None, "{label} has no ratio");
            assert_eq!(receipt.observed_samples, None, "{label} has no extent");
            assert_eq!(receipt.availability, "invalid_measurement");
            assert_eq!(receipt.speech_producer, CAPTURE_ENERGY_PRODUCER);
        }

        // The three receipts are genuinely different documents, not one status
        // rendered three ways.
        assert_ne!(silent.status, invalid.status);
        assert_eq!(
            invalid.status, mixed.status,
            "both invalid captures name the same reason; only the diagnostic \
             prefix differs, and that never reaches the receipt"
        );

        // Finality agrees with the receipts.
        assert_eq!(
            invalid_state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_terminal("rc-w3-quality-invalid", 0),
            Err(SealRefusal::CoverageIncomplete),
            "invalid measurement refuses the terminal seal"
        );
        assert_eq!(
            mixed_state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_terminal("rc-w3-quality-mixed", 0),
            Err(SealRefusal::CoverageIncomplete),
            "a valid second in front of an invalid one does not rescue the seal"
        );
    }

    /// rc-w3-acoustic-validity: the extent guard is not Silero-specific.
    ///
    /// The capture energy ladder sits upstream of the retained buffer, so in
    /// production it cannot fall behind it. This pins the guard's second arm
    /// anyway: whichever observer answers, an extent shorter than the capture
    /// is reported as an unobserved remainder rather than covered silence.
    #[test]
    fn a_short_capture_energy_extent_cannot_certify_the_unheard_remainder() {
        let state = state_for("rc-w3-validity-short-ladder", 10.0);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![0.0f32; at(4.0) as usize]);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            evidence.availability(),
            AcousticAvailability::Discontinuous {
                observed_samples: at(4.0)
            },
            "four measured seconds of a ten-second capture leave six unheard"
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let receipt = publish_terminal_coverage(&state, &tx);
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::PartialObservation)
        );
        assert_eq!(receipt.coverage_ratio(), None);
    }

    /// The terminal coverage threshold is 250 ms of the capture clock, and the
    /// published receipt carries the exact figure it was judged against.
    #[test]
    fn terminal_coverage_threshold_stays_at_two_hundred_fifty_milliseconds() {
        assert_eq!(SEAL_COVERAGE_INCOMPLETE_MS, 250);
        assert_eq!(
            u64::from(48_000_u32) * SEAL_COVERAGE_INCOMPLETE_MS / 1_000,
            12_000,
            "the immutable ledger threshold at 48 kHz"
        );
        assert_eq!(
            u64::from(RATE) * SEAL_COVERAGE_INCOMPLETE_MS / 1_000,
            THRESHOLD
        );

        let mut state = one_burst_in_a_ten_second_take("rc-w2-threshold");
        let (tx, _rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);
        assert_eq!(
            publish_terminal_coverage(&state, &tx).incomplete_threshold_samples,
            THRESHOLD
        );
        assert_eq!(
            repair_terminal_seal_coverage(&mut state, &tx, None, &LocalExecutionOwner::default())
                .incomplete_threshold_samples,
            THRESHOLD
        );
    }

    /// A sealed occurrence is finished. A later machine observation of the same
    /// PCM is refused as a replay, and the committed label does not move.
    #[test]
    fn a_sealed_occurrence_refuses_a_later_machine_observation() {
        // Three seconds, so the committed burst's padded range is inside the
        // retained PCM the qualification step has to read.
        let mut state = state_for("rc-w2-post-seal", 3.0);
        let (tx, _rx) = mpsc::unbounded_channel();
        let occurrence = commit_the_burst(&mut state, &tx);

        assert!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .seal_of(&occurrence)
                .is_some(),
            "the fixture must reach a seal before the replay means anything"
        );

        let replay = admit_ledger_label(
            &mut state,
            &tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    2,
                    0,
                    occurrence.clone(),
                ),
                label: "Iwo poprawione",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );

        assert!(matches!(
            replay,
            Some(MutationReceipt::Refuse {
                reason: RefuseReason::SealedReplay,
                ..
            })
        ));
        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .text_of(&occurrence)
                .map(str::to_owned),
            Some("Iwo".to_string()),
            "a refused replay may not rewrite sealed text"
        );
    }

    /// The retired char-diff verdict is diagnostics. A `skipped` job still
    /// forwards its payload, and that payload still reaches occurrence
    /// admission — proven end to end, through the two production functions that
    /// carry it, not through the receipt struct alone.
    #[test]
    fn a_legacy_skip_verdict_still_reaches_ledger_admission() {
        let mut state = state_for("rc-w2-legacy-skip", 2.0);
        let (tx, _rx) = mpsc::unbounded_channel();
        let occurrence =
            OccurrenceIdentity::new(state.session_id.clone(), state.capture_epoch, 0, at(1.0));
        let range = TailSampleRange {
            session: state.session_id.clone(),
            capture_epoch: state.capture_epoch,
            sample_start: 0,
            sample_end: at(1.0),
        };

        // Qualify the occurrence and open exactly the Whisper slot the job is
        // about, without pre-admitting any label.
        {
            let calibration = state.energy_calibration.clone().unwrap();
            let window = state.window_by_samples(0, at(1.0)).unwrap();
            let energy_integral = window
                .samples
                .iter()
                .map(|sample| f64::from(*sample) * f64::from(*sample))
                .sum::<f64>();
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: occurrence.clone(),
                            duration_ms: 1_000.0,
                            energy_integral,
                            mean_rms_dbfs: -12.0,
                            peak_dbfs: -12.0,
                            vad_open_sample: Some(0),
                            vad_close_sample: Some(at(1.0)),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration,
                    )
                    .is_qualified()
            );
            ledger.schedule_frontier(occurrence.clone(), vec![LedgerObservationProducer::Whisper]);
        }
        state.pending_events.insert(
            9,
            PendingAppleSeal {
                occurrence: occurrence.clone(),
                raw_text: "Iwo".into(),
                layer1_baseline: "Iwo".into(),
                start_ts: 0.0,
                end_ts: 1.0,
                segments: Vec::new(),
            },
        );
        let (tail_tx, mut tail_rx) = mpsc::channel(1);
        state.tail_patch = Some(tail_tx);
        let audio = state.window_by_samples(0, at(1.0)).unwrap().samples;
        assert!(state.queue_layer1_flush(
            &tx,
            CoalesceFlush {
                audio,
                committed_text: "Iwo".into(),
                member_ids: vec![(9, 1.0)],
                member_occurrences: vec![(9, occurrence.clone())],
                neighbour_context: String::new(),
                sample_start: 0,
                sample_end: at(1.0),
                admit_sample_start: 0,
                admit_sample_end: at(1.0),
                primary_utterance_id: 9,
            },
        ));
        let request = tail_rx.try_recv().expect("legally submitted request");
        request
            .provider_request
            .validate_pcm(&request.audio)
            .unwrap();
        let identity = request.provider_request.identity.clone();
        assert_eq!(state.refinement_submitted.len(), 1);
        assert_eq!(state.tail_patch_awaiting_completion, 1);

        let payload = TailProviderPayload {
            identity: identity.clone(),
            text: "odzysk".into(),
            segments: vec![TimedTailSegment {
                grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                text: "odzysk".into(),
                range: range.clone(),
            }],
            avg_logprob: None,
            compression_ratio: None,
            provider_id: crate::stt::tail_provider::TailProviderId::Fake,
            elapsed_ms: 0,
            evidence: crate::stt::tail_provider::TailProviderEvidence {
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                source: crate::stt::tail_provider::TailEvidenceSource::Whisper,
                revision: Some("rc-w2".into()),
                stability: crate::stt::tail_provider::TailEvidenceStability::Final,
                timing_quality: crate::stt::tail_provider::TailTimingQuality::Synthetic,
                avg_logprob: None,
            },
        };

        let mut lane =
            AppleTailPatchLane::new(RATE, None, crate::stt::tail_provider::TailProviderId::Fake);
        let completion = lane.finish_for_worker(
            Some(TailPatchInFlight {
                utterance_id: 9,
                request_identity: identity,
                admit_sample_start: occurrence.sample_start,
                admit_sample_end: occurrence.sample_end,
                member_occurrences: vec![(9, occurrence.clone())],
            }),
            Ok(TailPatchJobResult {
                utterance_id: 9,
                outcome: TailPatchOutcome::skipped(
                    SkipReasonCode::ChangeRatio,
                    "ratio 1.31 exceeds max 0.50",
                ),
                payload,
            }),
        );

        assert!(
            completion.payload.is_some(),
            "a skipped verdict must not discard the provider payload"
        );

        state.complete_whisper_window(&tx, completion, 1.0);

        assert_eq!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .text_of(&occurrence)
                .map(str::to_owned),
            Some("odzysk".to_string()),
            "the payload a legacy skip 'rejected' is the one the ledger admitted"
        );
        assert_eq!(state.tail_patch_jobs_applied, 1);
        assert_eq!(state.tail_patch_jobs_skipped, 0);
    }

    /// Ported from the parked `mod tests`
    /// (`apple_segments_map_to_captured_pcm_samples_at_ingestion`): Apple
    /// segment seconds land on the session PCM clock at ingestion, and a
    /// segment reaching past captured audio is clamped rather than inventing
    /// samples. The current owner is `apple_segments_on_pcm_clock`.
    #[test]
    fn apple_segments_land_on_the_captured_pcm_clock() {
        let state = state_for("rc-w2-segments", 2.0);
        let mapped = apple_segments_on_pcm_clock(
            &state,
            &[
                TranscriptSegment {
                    text: "Iwo".into(),
                    start_ts: 0.25,
                    end_ts: 0.75,
                },
                TranscriptSegment {
                    text: "poza".into(),
                    start_ts: 1.5,
                    end_ts: 9.0,
                },
            ],
        );
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].range.sample_start, at(0.25));
        assert_eq!(mapped[0].range.sample_end, at(0.75));
        assert_eq!(mapped[0].range.session, "rc-w2-segments");
        assert_eq!(mapped[1].range.sample_start, at(1.5));
        assert_eq!(
            mapped[1].range.sample_end,
            at(2.0),
            "a segment cannot claim audio the session never captured"
        );
    }

    // ══════════════════════════════════════════════════════════════════
    // Ported from the parked `mod tests`: engine lifecycle over the single
    // Silero. `EpochGate`, `EpochDecision`, `EPOCH_PREROLL_SECS` and every
    // method used below are live production symbols with a live consumer
    // (`apple_stream_worker`); only the parked module around them was dead, so
    // these contracts move here unchanged rather than being retired.
    // ══════════════════════════════════════════════════════════════════

    /// Amplitude stand-in for the session Silero's `speech_live` bit, so the
    /// epoch state machine can be driven on synthetic PCM without loading the
    /// VAD model (a unit test must not depend on the model being present).
    fn amplitude_edge(samples: &[f32], threshold: f32) -> bool {
        samples.iter().any(|s| s.abs() >= threshold)
    }

    /// 200 Hz tone at `amplitude` — the "speech" side of the fixture.
    fn tone(secs: f32, amplitude: f32) -> Vec<f32> {
        let total = (secs * RATE as f32) as usize;
        (0..total)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                amplitude * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
            })
            .collect()
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * RATE as f32) as usize]
    }

    /// Drive the gate the way the worker does — chunk by chunk — keeping each
    /// decision next to the session cursor it was taken at. The last chunk of a
    /// block is short, so cursors are carried, never reconstructed from indices.
    fn drive(
        gate: &mut EpochGate,
        audio: &[f32],
        samples_seen: &mut u64,
    ) -> Vec<(u64, EpochDecision)> {
        let mut out = Vec::new();
        for chunk in audio.chunks(1024) {
            *samples_seen += chunk.len() as u64;
            out.push((
                *samples_seen,
                gate.feed_pcm(chunk, *samples_seen, amplitude_edge(chunk, 0.1)),
            ));
        }
        out
    }

    /// Speech opens an epoch, silence past the product threshold closes it, and
    /// the next speech edge wakes a new one whose base carries the pre-roll —
    /// without reaching back into the epoch that already closed.
    #[test]
    fn epoch_gate_sleeps_after_threshold_silence_and_wakes_with_preroll() {
        let mut gate = EpochGate::armed(RATE, 5.0);
        let mut seen = 0u64;

        let speech = drive(&mut gate, &tone(2.0, 0.5), &mut seen);
        assert!(
            matches!(
                speech.first(),
                Some((_, EpochDecision::Wake { preroll_from: 0 }))
            ),
            "first speech chunk must open epoch 0 (nothing was retained before it), got {:?}",
            speech.first()
        );
        assert!(
            speech[1..]
                .iter()
                .all(|(_, d)| *d == EpochDecision::Forward),
            "speech after the wake must forward, got {:?}",
            &speech[1..]
        );

        let quiet = drive(&mut gate, &silence(6.0), &mut seen);
        let sleep_at = quiet
            .iter()
            .position(|(_, d)| matches!(d, EpochDecision::Sleep { .. }))
            .expect("6 s of silence at a 5 s threshold must close the epoch");
        let (sleep_cursor, EpochDecision::Sleep { silence_secs }) = quiet[sleep_at] else {
            unreachable!("position() matched Sleep");
        };
        assert!(
            (5.0..5.2).contains(&silence_secs),
            "the epoch must close within one chunk of the 5 s threshold, closed at {silence_secs}s"
        );
        assert!(
            quiet[sleep_at + 1..]
                .iter()
                .all(|(_, d)| *d == EpochDecision::Idle),
            "after sleeping the engine rests until the next speech edge, got {:?}",
            &quiet[sleep_at + 1..]
        );

        let resume_cursor = seen;
        let woke = drive(&mut gate, &tone(1.0, 0.5), &mut seen);
        let (_, EpochDecision::Wake { preroll_from }) = woke[0] else {
            panic!("speech after rest must wake a new epoch, got {:?}", woke[0]);
        };
        let preroll = (EPOCH_PREROLL_SECS * RATE as f32) as u64;
        assert_eq!(
            preroll_from,
            resume_cursor.saturating_sub(preroll),
            "the new epoch is based one pre-roll ahead of the chunk that woke it"
        );
        assert!(
            preroll_from >= sleep_cursor,
            "pre-roll must not re-feed audio the closed epoch already carried \
             ({preroll_from} < {sleep_cursor})"
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
            decisions.iter().all(|(_, d)| *d == EpochDecision::Forward),
            "disarmed gate must forward every chunk, got {:?}",
            decisions
                .iter()
                .filter(|(_, d)| *d != EpochDecision::Forward)
                .collect::<Vec<_>>()
        );
    }

    /// No Silero means no edges, and an armed gate with no edge source would
    /// rest forever on a stream that never opened. The lifecycle fails open.
    #[test]
    fn epoch_gate_without_speech_edges_falls_back_to_one_stream() {
        let mut gate = EpochGate::for_session(RATE, Some(5.0), false);
        assert!(
            !gate.is_armed(),
            "an armed gate with no edge source would sleep the engine forever"
        );
        assert_eq!(
            gate.feed_pcm(&[0.0; 1_024], 1_024, false),
            EpochDecision::Forward,
            "Silero absence must preserve continuous Apple PCM flow"
        );
        assert!(EpochGate::for_session(RATE, Some(5.0), true).is_armed());
        assert!(
            !EpochGate::for_session(RATE, None, true).is_armed(),
            "no hands-free silence setting is still the legacy single stream"
        );
    }

    /// Ported from the parked `mod tests`
    /// (`epoch_shift_lifts_segment_times_onto_the_session_pcm_clock`): bridge
    /// time restarts at zero for every epoch, so events leaving a non-zero
    /// epoch must be lifted onto the session PCM clock before any seal maps
    /// them to samples. Only the retired `AppleSealState::new` constructor kept
    /// this test parked; the contract and both owners are live.
    #[test]
    fn epoch_shift_lifts_segment_times_onto_the_session_pcm_clock() {
        let state = state_for("rc-w2-epoch-shift", 110.0);

        let shifted = shift_events(
            vec![LiveStreamEvent::PhraseFinal {
                text: "Iwo".into(),
                segments: vec![TranscriptSegment {
                    text: "Iwo".into(),
                    start_ts: 0.5,
                    end_ts: 2.0,
                }],
            }],
            100.0,
        );
        let LiveStreamEvent::PhraseFinal { segments, .. } = &shifted[0] else {
            panic!(
                "the shim must preserve the event kind, got {:?}",
                shifted[0]
            );
        };
        // Both sums are exact in binary32, so this is equality, not tolerance.
        assert_eq!(segments[0].start_ts, 100.5);
        assert_eq!(segments[0].end_ts, 102.0);

        let on_pcm = apple_segments_on_pcm_clock(&state, segments);
        assert_eq!(on_pcm[0].range.sample_start, at(100.5));
        assert_eq!(on_pcm[0].range.sample_end, at(102.0));
    }

    /// Ported from the parked `mod tests`
    /// (`epoch_shift_at_base_zero_is_identity`): the first epoch is based at 0,
    /// so the shim is the identity there. This is what keeps a single-epoch
    /// take bit-identical to the legacy one-stream lane.
    #[test]
    fn epoch_shift_at_base_zero_is_identity() {
        let shifted = shift_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "Iwo".into(),
                    segments: vec![TranscriptSegment {
                        text: "Iwo".into(),
                        start_ts: 0.5,
                        end_ts: 2.0,
                    }],
                },
                LiveStreamEvent::Summary {
                    text: "Iwo wraca".into(),
                    segments: vec![TranscriptSegment {
                        text: "Iwo wraca".into(),
                        start_ts: 0.5,
                        end_ts: 4.0,
                    }],
                    ok: true,
                    error: None,
                },
            ],
            0.0,
        );
        let LiveStreamEvent::Partial { segments, .. } = &shifted[0] else {
            panic!(
                "the shim must preserve the event kind, got {:?}",
                shifted[0]
            );
        };
        assert_eq!((segments[0].start_ts, segments[0].end_ts), (0.5, 2.0));
        let LiveStreamEvent::Summary { segments, .. } = &shifted[1] else {
            panic!(
                "the shim must preserve the event kind, got {:?}",
                shifted[1]
            );
        };
        assert_eq!((segments[0].start_ts, segments[0].end_ts), (0.5, 4.0));
    }

    // ══════════════════════════════════════════════════════════════════
    // Ported from the parked `mod tests`: phrase-restart adjudication.
    // `phrase_restart_should_freeze_prior` is live production
    // (`apple_live_session.rs:3639`) with a live consumer inside
    // `emit_stream_events`, and every test of it was parked — so this rule
    // currently ships unguarded. Nothing here revives a retired helper: the
    // parked bodies only needed the current `AppleSealState` constructor.
    // ══════════════════════════════════════════════════════════════════

    fn segment(text: &str, start_ts: f32, end_ts: f32) -> TranscriptSegment {
        TranscriptSegment {
            text: text.into(),
            start_ts,
            end_ts,
        }
    }

    /// Ported unchanged from the parked `mod tests`: the measured phrase-restart
    /// vectors, including the 40→20 collapse the old rule missed. The table is
    /// the falsifier — this test fails if any vector's verdict moves, and fails
    /// if a vector is silently dropped from the fixture.
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

    /// Ported from the parked `mod tests`: after a long open partial, SFSpeech
    /// collapses onto the next sentence's shared opener. That collapse must
    /// freeze the prior utterance — the old rule did not, and whole sentences
    /// disappeared.
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

    /// Ported from the parked `mod tests`: revisions and rewinds retain the
    /// prior text; only a forward extension containing the full prior
    /// hypothesis may replace it.
    #[test]
    fn utterance_drop_revision_and_rewind_retain_prior() {
        // 95 → 79 char mid-reword classifies as a revision, and still freezes,
        // because otherwise its removed span has no retained copy anywhere.
        let prev = format!("{}MIDDLE{}", "x".repeat(50), "y".repeat(39));
        let next = format!("{}REVISE{}", "x".repeat(50), "y".repeat(23));
        assert_eq!(prev.len(), 95);
        assert_eq!(next.len(), 79);
        assert!(
            phrase_restart_should_freeze_prior(&prev, &next),
            "revision must retain the prior hypothesis"
        );
        assert!(!phrase_restart_should_freeze_prior(
            "Zdanie",
            "Zdanie szóste spokojnie"
        ));
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

    /// Ported from the parked `mod tests`: the same rule at the adjudication
    /// layer, through the live `emit_stream_events` consumer — a partial
    /// sequence that used to drop the post-stressor sentence now seals it as
    /// `UtteranceFinal` before the restart partial lands as a preview.
    #[test]
    fn utterance_drop_emit_seals_prior_on_shared_opener_partial_restart() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state_for("rc-w2-utterance-drop", 30.0);
        // The port needs the real physical qualification owner. Merely setting
        // calibration leaves the fallback lane unqualified and emits no seals.
        let mut fusion = SileroIngress::new(RATE, state.session_id.clone(), state.capture_epoch);
        for (start, end) in [(0, 5), (5, 10), (10, 15)] {
            fusion.observe(
                Some((start * u64::from(RATE), end * u64::from(RATE))),
                true,
                end * u64::from(RATE),
            );
        }
        state.fusion = Some(fusion);
        state.fusion_seal_armed = true;
        let s5 = "Zdanie piąte, szybko bez pauz. Teraz mówię bardzo szybko, bez żadnej przerwy, \
                  żeby sprawdzić czy silnik nadąża za tempem, którego normalnie unika w \
                  codziennym dyktowaniu.";
        let s6 = "Zdanie szóste spokojnie po stresie wracam do normalnego tempa i mówię wyraźnie.";
        let s7 = "Zdanie siódme Overlap cztery angielskie terminy w polskim";
        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: s5.to_string(),
                    segments: vec![segment(s5, 0.0, 5.0)],
                },
                // The stressor phrase seals cleanly.
                LiveStreamEvent::PhraseFinal {
                    text: s5.to_string(),
                    segments: vec![segment(s5, 0.0, 5.0)],
                },
                // The post-stressor sentence builds as an open partial…
                LiveStreamEvent::Partial {
                    text: s6.to_string(),
                    segments: vec![segment(s6, 5.0, 10.0)],
                },
                // …then SFSpeech restarts onto the next opener without isFinal.
                LiveStreamEvent::Partial {
                    text: "Zdanie".to_string(),
                    segments: vec![segment("Zdanie", 10.0, 10.5)],
                },
                LiveStreamEvent::Partial {
                    text: s7.to_string(),
                    segments: vec![segment(s7, 10.0, 15.0)],
                },
                LiveStreamEvent::PhraseFinal {
                    text: s7.to_string(),
                    segments: vec![segment(s7, 10.0, 15.0)],
                },
            ],
            &tx,
            &mut state,
            30.0,
        );
        drop(tx);
        let mut finals = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::UtteranceFinal { text, .. } = event {
                finals.push(text);
            }
        }
        assert!(
            finals
                .iter()
                .any(|t| t.contains("szóste") || t.contains("szost")),
            "the post-stressor sentence must be committed, got finals: {finals:?}"
        );
        assert!(
            finals
                .iter()
                .any(|t| t.contains("siódme") || t.contains("siodm") || t.contains("Overlap")),
            "the restarting sentence must still seal, got finals: {finals:?}"
        );
        assert!(
            state.sealed_count >= 3,
            "s5 + frozen s6 + s7 → at least 3 seals, got {}",
            state.sealed_count
        );
    }

    /// rc-w3-acoustic-validity: invalid PCM *after* valid PCM, through the real
    /// writer -> owner -> ledger chain.
    ///
    /// One second of finite zeros, then one second of NaN, on one bound
    /// accumulator. The reader used to refuse only when the non-finite count
    /// reached the whole observed extent (`16_000 >= 32_000` is false), so the
    /// NaN second was measured as silence, the hop was published with a zero
    /// RMS, and an empty ledger certified the take Complete.
    #[test]
    fn invalid_pcm_after_valid_pcm_is_not_certified_as_silence() {
        let state = state_for("rc-w3-validity-after", 2.0);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![0.0f32; at(1.0) as usize]);
        writer.push_samples(&vec![f32::NAN; at(1.0) as usize]);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
        assert_eq!(
            evidence.availability().as_str(),
            "invalid_measurement",
            "a NaN second measured nothing, and a valid second in front of it \
             does not turn it into silence"
        );
        assert!(!evidence.is_observed());
        assert!(evidence.ranges().is_empty());

        let receipt = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &evidence,
                THRESHOLD,
            )
        };
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement),
            "an unmeasurable region may not reach a successful seal"
        );
        assert_eq!(receipt.coverage_ratio(), None);
        assert_eq!(receipt.observed_samples, None);
    }

    /// rc-w3-acoustic-validity: invalid PCM *before* valid PCM.
    ///
    /// The mirror case, with infinities rather than NaN — `is_finite` rejects
    /// both, and both are substituted with zero before measurement. The valid
    /// tail's speech may not be published on evidence whose head was never
    /// measurable.
    #[test]
    fn invalid_pcm_before_valid_pcm_is_not_certified_as_silence() {
        let state = state_for("rc-w3-validity-before", 2.0);
        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![f32::NEG_INFINITY; at(1.0) as usize]);
        writer.push_samples(&vec![0.25f32; at(1.0) as usize]);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(
            evidence.availability().as_str(),
            "invalid_measurement",
            "an infinite head is as unmeasurable as a NaN one"
        );
        assert!(
            evidence.ranges().is_empty(),
            "unavailable evidence publishes no speech, not even the valid tail's"
        );

        let receipt = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &evidence,
                THRESHOLD,
            )
        };
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement)
        );
    }

    /// rc-w3-acoustic-validity: a measuring observer cannot overrule the
    /// capture owner's invalid verdict on the same PCM.
    ///
    /// Silero reports a crossing over a take whose capture writer measured
    /// nothing but NaN. Selection preferred any observer that "measured
    /// speech", so the downstream observer won and the invalid capture never
    /// reached the ledger.
    #[test]
    fn a_measuring_observer_cannot_overrule_an_invalid_capture() {
        let state = one_burst_in_a_ten_second_take("rc-w3-validity-bypass");
        assert!(
            coverage_speech_evidence(&state).observed_speech(),
            "the fixture must start with a downstream observer that measured speech"
        );

        let mut writer = CaptureLevelAccumulator::bound_to(&state.capture_energy);
        writer.push_samples(&vec![f32::NAN; at(10.0) as usize]);

        let evidence = coverage_speech_evidence(&state);
        assert_eq!(
            evidence.producer(),
            CAPTURE_ENERGY_PRODUCER,
            "the capture owner adjudicates the PCM it wrote; no later observer \
             may certify audio the writer measured as invalid"
        );
        assert_eq!(evidence.availability().as_str(), "invalid_measurement");

        let receipt = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &evidence,
                THRESHOLD,
            )
        };
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::InvalidMeasurement)
        );
    }

    /// rc-w3-acoustic-validity: a partly observed capture cannot certify its
    /// unobserved tail, even when no word was committed inside that tail.
    ///
    /// Forwarding stops after four seconds of a ten-second take. No chunk ever
    /// skips, so nothing is discontinuous — the observer's extent simply ends
    /// early. Its one measured burst is committed, so the coverage arithmetic
    /// is perfect and the six unheard seconds were the whole lie: the ledger
    /// compared committed and measured spans against the observer's extent and
    /// never against the capture the take actually produced.
    #[test]
    fn a_partly_observed_capture_cannot_certify_its_unobserved_tail() {
        let mut state = state_for("rc-w3-validity-tail", 10.0);
        let mut ingress = SileroIngress::new(RATE, state.session_id.clone(), 0);
        ingress.note_observed_pcm(at(4.0), at(4.0));
        ingress.observe_boundaries(&[
            crossing(VadBoundaryKind::SpeechStart, at(1.0)),
            crossing(VadBoundaryKind::SpeechEnd, at(2.0)),
        ]);
        ingress.observe(Some((0, at(4.0))), true, at(4.0));
        state.fusion = Some(ingress);

        let (tx, _rx) = mpsc::unbounded_channel();
        commit_the_burst(&mut state, &tx);

        let evidence = coverage_speech_evidence(&state);
        let receipt = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.assess_seal_coverage(
                &state.session_id,
                state.capture_epoch,
                &evidence,
                THRESHOLD,
            )
        };
        assert_ne!(
            receipt.status,
            SealCoverageStatus::Complete,
            "six seconds of this capture never reached any observer; covering \
             the measured burst does not certify them silent"
        );
        assert_eq!(
            receipt.status,
            SealCoverageStatus::Unavailable(AcousticEvidenceGap::PartialObservation)
        );
        assert_eq!(receipt.coverage_ratio(), None);
    }
}

/// rc-w2-test-rehab: current-owner replacements for the 26 parked contracts.
/// Synthetic PCM/edges below are unit fixtures, never measured take receipts.
/// Raw finals are telemetry; every document assertion reads AcousticLedger.
#[cfg(test)]
mod rc_w2_test_rehab {
    use super::*;
    use crate::pipeline::acoustic_ledger::RefuseReason;

    const RATE: u32 = 16_000;

    fn sample(secs: f32) -> u64 {
        (secs * RATE as f32).round() as u64
    }

    fn segment(text: &str, start_ts: f32, end_ts: f32) -> TranscriptSegment {
        TranscriptSegment {
            text: text.into(),
            start_ts,
            end_ts,
        }
    }

    fn state(session: &str, secs: f32) -> AppleSealState {
        let mut state = AppleSealState::new_for_session(RATE, session.into(), 7);
        state.energy_calibration = Some(EnergyCalibration::new("rehab-synthetic", 1.0, 1));
        for chunk in vec![0.25; sample(secs) as usize].chunks(1024) {
            state.audio.push(chunk);
        }
        state
    }

    // The fallback Apple path consumes an EXISTING qualification. Supply that
    // exact precondition from this fixture's retained PCM, without admitting a
    // label or manufacturing a terminal coverage receipt.
    fn qualify(state: &mut AppleSealState, start: f32, end: f32) -> OccurrenceIdentity {
        let occurrence = OccurrenceIdentity::new(
            &state.session_id,
            state.capture_epoch,
            sample(start),
            sample(end),
        );
        let window = state
            .window_by_samples(sample(start), sample(end))
            .expect("fixture PCM");
        assert!(window.samples.iter().all(|value| *value == 0.25));
        let calibration = state
            .energy_calibration
            .as_ref()
            .expect("fixture calibration");
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: f64::from(end - start) * 1_000.0,
            energy_integral: window.samples.len() as f64 * 0.25_f64.powi(2),
            mean_rms_dbfs: 20.0 * 0.25_f64.log10(),
            peak_dbfs: 20.0 * 0.25_f64.log10(),
            vad_open_sample: Some(sample(start)),
            vad_close_sample: Some(sample(end)),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(
            state
                .acoustic_ledger
                .lock()
                .unwrap()
                .qualify(&evidence, calibration)
                .is_qualified()
        );
        occurrence
    }

    fn physical_state(session: &str, secs: f32, ranges: &[(f32, f32)]) -> AppleSealState {
        let mut state = state(session, secs);
        let mut fusion = SileroIngress::new(RATE, session, state.capture_epoch);
        for &(start, end) in ranges {
            let observed = fusion.observe(Some((sample(start), sample(end))), true, sample(end));
            assert_eq!(observed.closed.len(), 1);
        }
        state.fusion = Some(fusion);
        state.fusion_seal_armed = true;
        state.fusion_context = FusionContextMode::UtteranceOnly;
        state
    }

    fn emit(
        state: &mut AppleSealState,
        tx: &mpsc::UnboundedSender<EngineEvent>,
        words: Vec<TranscriptSegment>,
    ) {
        let text = words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let secs = state.audio.session_sample_end() as f32 / RATE as f32;
        emit_stream_events(
            vec![LiveStreamEvent::PhraseFinal {
                text,
                segments: words,
            }],
            tx,
            state,
            secs,
        );
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn document(state: &AppleSealState) -> String {
        state.acoustic_ledger.lock().unwrap().rendered_text()
    }

    fn raw_finals(events: &[EngineEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { raw_text, .. } => Some(raw_text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn count_iwo(text: &str) -> usize {
        text.split_whitespace()
            .filter(|word| word.eq_ignore_ascii_case("iwo"))
            .count()
    }

    #[test]
    fn emit_maps_partial_and_two_phrase_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("two-finals", 2.0, &[(0.0, 1.0), (1.0, 2.0)]);
        emit_stream_events(
            vec![LiveStreamEvent::Partial {
                text: "hello".into(),
                segments: vec![segment("hello", 0.0, 0.5)],
            }],
            &tx,
            &mut state,
            0.5,
        );
        assert!(document(&state).is_empty(), "preview cannot admit words");
        emit(&mut state, &tx, vec![segment("hello world", 0.0, 1.0)]);
        emit(&mut state, &tx, vec![segment("second", 1.0, 2.0)]);
        let events = drain(&mut rx);
        assert!(matches!(&events[0], EngineEvent::Preview { rev: 1, text, .. } if text == "hello"));
        let ids = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal { utterance_id, .. } => Some(*utterance_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(state.sealed_count, 2);
        assert_eq!(document(&state), "hello world second");
    }

    /// The old test synthesized a new window from stale timing and novel text.
    /// Current contract: novelty alone is not PCM identity. A later exact
    /// observation preserves the novel phrase without rewriting the first one.
    #[test]
    fn boundary_consumed_final_with_novel_text_requires_exact_pcm() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("stale-final", 4.0, &[(0.0, 1.0), (2.0, 3.0)]);
        emit(&mut state, &tx, vec![segment("prior", 0.0, 1.0)]);
        drain(&mut rx);
        emit(&mut state, &tx, vec![segment("novel phrase", 0.0, 1.0)]);
        assert_eq!(document(&state), "prior");
        assert!(raw_finals(&drain(&mut rx)).is_empty());
        emit(&mut state, &tx, vec![segment("novel phrase", 2.0, 3.0)]);
        assert_eq!(document(&state), "prior novel phrase");
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().occurrences().count(),
            2
        );
    }

    /// Untimed preview cannot be promoted into committed speech. Preserve the
    /// refusal and prove that the same words with actual timing can still land.
    #[test]
    fn frozen_partial_without_segments_requires_exact_pcm() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("untimed-freeze", 3.0, &[(0.0, 2.0)]);
        let prior = "whole prior utterance retained for exact observation";
        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: prior.into(),
                    segments: Vec::new(),
                },
                LiveStreamEvent::Partial {
                    text: "Next".into(),
                    segments: Vec::new(),
                },
            ],
            &tx,
            &mut state,
            2.0,
        );
        let events = drain(&mut rx);
        assert!(events.iter().any(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == "apple_final_without_pcm_timing"
        )));
        assert!(raw_finals(&events).is_empty());
        assert!(document(&state).is_empty());
        emit(&mut state, &tx, vec![segment(prior, 0.0, 2.0)]);
        assert_eq!(document(&state), prior);
    }

    /// Retention now preserves the bounded tail for terminal gap repair; the
    /// old immediate-release assertion would destroy required recovery PCM.
    #[test]
    fn seals_resolve_their_audio_window_from_retained_pcm() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("retained-seals", 6.0);
        qualify(&mut state, 0.5, 2.0);
        qualify(&mut state, 2.5, 4.0);
        emit(&mut state, &tx, vec![segment("first", 0.5, 2.0)]);
        emit(&mut state, &tx, vec![segment("second", 2.5, 4.0)]);
        assert_eq!(state.sealed_count, 2);
        assert_eq!(raw_finals(&drain(&mut rx)), vec!["first", "second"]);
        assert_eq!(state.unresolved_windows, 0);
        assert_eq!(state.last_sealed_end, 4.0);
        assert_eq!(
            state.audio.window(0.0, 1.0).unwrap(),
            vec![0.25; RATE as usize]
        );
        assert!(state.audio.window(2.5, 4.0).is_some());
    }

    #[test]
    fn cumulative_apple_final_commits_only_segments_after_last_boundary() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("cumulative-segments", 4.0);
        qualify(&mut state, 0.0, 2.0);
        qualify(&mut state, 2.0, 3.0);
        emit(
            &mut state,
            &tx,
            vec![segment("alpha", 0.0, 1.0), segment("beta", 1.0, 2.0)],
        );
        emit(
            &mut state,
            &tx,
            vec![
                segment("alpha", 0.0, 1.0),
                segment("beta", 1.0, 2.0),
                segment("gamma", 2.0, 3.0),
            ],
        );
        let events = drain(&mut rx);
        let finals = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::UtteranceFinal {
                    raw_text,
                    start_ts,
                    end_ts,
                    ..
                } => Some((raw_text.as_str(), *start_ts, *end_ts)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(finals, vec![("alpha beta", 0.0, 2.0), ("gamma", 2.0, 3.0)]);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,
                    EngineEvent::Warning { code, .. } if code == APPLE_FINAL_OVERLAP_WARNING_CODE
                ))
                .count(),
            1
        );
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().occurrences().count(),
            2
        );
    }

    #[test]
    fn cumulative_final_commits_only_its_exact_novel_suffix() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("novel-suffix", 3.0, &[(0.0, 2.0), (2.0, 3.0)]);
        emit(&mut state, &tx, vec![segment("alpha beta", 0.0, 2.0)]);
        emit(
            &mut state,
            &tx,
            vec![segment("alpha beta revised", 0.0, 2.0)],
        );
        assert_eq!(
            document(&state),
            "alpha beta",
            "stale span cannot authenticate a suffix"
        );
        emit(
            &mut state,
            &tx,
            vec![
                segment("alpha beta", 0.0, 2.0),
                segment("revised", 2.0, 3.0),
            ],
        );
        assert_eq!(document(&state), "alpha beta revised");
        assert_eq!(raw_finals(&drain(&mut rx)), vec!["alpha beta", "revised"]);
    }

    #[test]
    fn five_spoken_occurrences_survive_a_cumulative_restatement_end_to_end() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ranges = (0..5)
            .map(|i| (i as f32, (i + 1) as f32))
            .collect::<Vec<_>>();
        let mut state = physical_state("five-restated", 5.0, &ranges);
        let words = ranges
            .iter()
            .map(|&(start, end)| segment("Iwo", start, end))
            .collect::<Vec<_>>();
        emit(&mut state, &tx, words[..4].to_vec());
        assert_eq!(count_iwo(&document(&state)), 4);
        emit(&mut state, &tx, words);
        assert_eq!(count_iwo(&document(&state)), 5);
        assert_eq!(raw_finals(&drain(&mut rx)), vec!["Iwo"; 5]);
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().occurrences().count(),
            5
        );
    }

    /// Prefix identity is acoustic even after Lexicon changes its spelling.
    /// Drive the configured seal path, then replay Apple against the sealed
    /// prefix: no text matcher is permitted to mint another occurrence.
    #[test]
    fn cumulative_final_prefix_survives_words_the_lexicon_rewrites() {
        with_lexicon_fixture(
            "cumulative_final_prefix_survives_words_the_lexicon_rewrites",
            || {
                let (tx, mut rx) = mpsc::unbounded_channel();
                let mut state = state("rewritten-prefix", 3.0);
                let occurrence = qualify(&mut state, 0.0, 2.0);
                emit(&mut state, &tx, vec![segment("uruchom doker", 0.0, 2.0)]);
                assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&occurrence));
                assert_eq!(document(&state), "uruchom Docker");
                state.fusion_seal_armed = true;
                let mut fusion =
                    SileroIngress::new(RATE, state.session_id.clone(), state.capture_epoch);
                fusion.observe(Some((0, sample(2.0))), true, sample(2.0));
                fusion.observe(Some((sample(2.0), sample(3.0))), true, sample(3.0));
                state.fusion = Some(fusion);
                emit(
                    &mut state,
                    &tx,
                    vec![
                        segment("uruchom doker", 0.0, 2.0),
                        segment("i restart", 2.0, 3.0),
                    ],
                );
                assert_eq!(document(&state), "uruchom Docker i restart");
                assert_eq!(
                    state.acoustic_ledger.lock().unwrap().occurrences().count(),
                    2
                );
                assert!(drain(&mut rx).iter().any(|event| matches!(
                    event,
                    EngineEvent::LedgerMutation {
                        receipt: MutationReceipt::Refuse { .. },
                        ..
                    }
                )));
            },
        );
    }

    /// The old explicit empty Preview event belongs to presentation. Here the
    /// current owner clears its volatile state and emits no duplicate final.
    #[test]
    fn fully_reheard_cumulative_final_clears_preview_instead_of_repeating_canvas() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("reheard", 3.0, &[(0.0, 2.0)]);
        emit(&mut state, &tx, vec![segment("heard phrase", 0.0, 2.0)]);
        drain(&mut rx);
        emit_stream_events(
            vec![LiveStreamEvent::Partial {
                text: "heard phrase".into(),
                segments: vec![segment("heard phrase", 0.0, 2.0)],
            }],
            &tx,
            &mut state,
            3.0,
        );
        assert_eq!(state.open_partial, "heard phrase");
        drain(&mut rx);
        emit(&mut state, &tx, vec![segment("heard phrase", 0.0, 2.0)]);
        let events = drain(&mut rx);
        assert!(state.open_partial.is_empty());
        assert!(state.open_partial_segments.is_empty());
        assert!(raw_finals(&events).is_empty());
        assert!(
            !events.iter().any(
                |event| matches!(event, EngineEvent::Preview { text, .. } if !text.is_empty())
            )
        );
        assert_eq!(document(&state), "heard phrase");
    }

    #[test]
    fn legitimate_repeated_words_survive_disjoint_apple_windows() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("repeated-tak", 3.0, &[(0.0, 1.0), (1.0, 2.0)]);
        emit(&mut state, &tx, vec![segment("tak", 0.0, 1.0)]);
        emit(&mut state, &tx, vec![segment("tak", 1.0, 2.0)]);
        assert_eq!(raw_finals(&drain(&mut rx)), vec!["tak", "tak"]);
        assert_eq!(document(&state), "tak tak");
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().occurrences().count(),
            2
        );
    }

    /// Out-of-capture text cannot seal without a nonempty qualified occurrence.
    #[test]
    fn seal_window_beyond_captured_audio_is_counted_unresolved() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("future-window", 2.0);
        emit(&mut state, &tx, vec![segment("future phrase", 8.0, 9.0)]);
        assert_eq!(state.unresolved_windows, 1);
        assert_eq!(state.last_sealed_end, 0.0);
        assert_eq!(state.sealed_count, 0);
        assert!(raw_finals(&drain(&mut rx)).is_empty());
        assert!(document(&state).is_empty());
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().occurrences().count(),
            0
        );
    }

    /// Real event producer and real channel, with sender still open at assert.
    /// No hand-written select loop masquerading as the session implementation.
    #[tokio::test]
    async fn live_previews_surface_before_audio_eof() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("preview-before-eof", 1.0);
        for text in ["a", "ab"] {
            emit_stream_events(
                vec![LiveStreamEvent::Partial {
                    text: text.into(),
                    segments: vec![segment(text, 0.0, 0.5)],
                }],
                &tx,
                &mut state,
                0.5,
            );
        }
        for (expected_rev, expected) in [(1, "a"), (2, "ab")] {
            let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("preview before EOF")
                .expect("producer stays open");
            assert!(
                matches!(event, EngineEvent::Preview { rev, text, .. } if rev == expected_rev && text == expected)
            );
        }
        assert!(!rx.is_closed());
        assert!(document(&state).is_empty());
        drop(tx);
    }

    fn only_preview(events: &[EngineEvent]) -> serde_json::Value {
        let previews = events
            .iter()
            .filter(|event| matches!(event, EngineEvent::Preview { .. }))
            .collect::<Vec<_>>();
        assert_eq!(previews.len(), 1, "one partial paints one preview");
        serde_json::to_value(previews[0]).unwrap()
    }

    /// Counterexample B (Roman, 2026-09-24): a partial with text and word
    /// segments became a Preview with no PCM range — the pins were dropped
    /// before the emitter painted. The preview must carry the range those
    /// segments occupy on the capture counter, at word grain.
    #[test]
    fn partial_with_segments_paints_its_capture_range() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("preview-pins", 2.0);
        emit_stream_events(
            vec![LiveStreamEvent::Partial {
                text: "dzień dobry".into(),
                segments: vec![segment("dzień", 0.25, 0.5), segment("dobry", 0.5, 1.0)],
            }],
            &tx,
            &mut state,
            1.0,
        );
        let preview = only_preview(&drain(&mut rx));
        assert_eq!(preview["text"], "dzień dobry");
        assert_eq!(
            preview["pin"]["range"]["sample_start"],
            sample(0.25),
            "the preview paint carries no PCM range: {preview}"
        );
        assert_eq!(preview["pin"]["range"]["sample_end"], sample(1.0));
        assert_eq!(preview["pin"]["range"]["session"], "preview-pins");
        assert_eq!(preview["pin"]["range"]["capture_epoch"], 7);
        assert_eq!(preview["pin"]["grain"], "word");
        assert!(document(&state).is_empty(), "a preview never admits words");
    }

    /// Counterexample B, second half: Swift may send a partial with text and
    /// no segments after filtering. That preview was painted with no receipt.
    /// It must paint the open occurrence's capture range as utterance grain
    /// with one named receipt — never invented per-word ranges.
    #[test]
    fn partial_without_segments_paints_the_open_occurrence_at_utterance_grain() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = state("preview-unpinned", 2.0);
        emit_stream_events(
            vec![LiveStreamEvent::Partial {
                text: "bez pinów".into(),
                segments: Vec::new(),
            }],
            &tx,
            &mut state,
            1.5,
        );
        let preview = only_preview(&drain(&mut rx));
        assert_eq!(preview["text"], "bez pinów");
        assert_eq!(
            preview["pin"]["grain"], "utterance",
            "a segment-less partial was painted with no grain or receipt: {preview}"
        );
        assert_eq!(preview["pin"]["receipt"], "partial_without_segments");
        assert_eq!(preview["pin"]["range"]["sample_start"], 0);
        assert_eq!(preview["pin"]["range"]["sample_end"], sample(1.5));
        assert!(document(&state).is_empty(), "a preview never admits words");
    }

    /// Run the same named test in an isolated process with a real custom table.
    /// Command::env changes only the child's launch environment. Other tests
    /// keep their own config, and the test never edits the Founder's dictionary.
    /// The marker chooses the child arm; it is private to this fresh tempdir.
    fn with_lexicon_fixture(name: &str, check: impl FnOnce()) {
        if let Some(root) = std::env::var_os("CODESCRIBE_DATA_DIR") {
            let root = std::path::PathBuf::from(root);
            if std::fs::read(root.join("rc-w2-lexicon-case"))
                .ok()
                .as_deref()
                == Some(name.as_bytes())
            {
                assert_eq!(
                    crate::config::Config::config_dir(),
                    root.canonicalize().unwrap()
                );
                assert_eq!(
                    crate::quality::overlay_quality::apply_custom_lexicon("doker"),
                    "Docker",
                    "positive control: the configured rule must change the input",
                );
                check();
                return;
            }
        }
        let directory = tempfile::tempdir().expect("isolated lexicon directory");
        std::fs::write(directory.path().join("rc-w2-lexicon-case"), name)
            .expect("child fixture marker");
        std::fs::write(
            directory.path().join("lexicon.custom.jsonl"),
            "{\"term\":\"Docker\",\"mispronunciations\":[\"doker\"],\"source\":\"manual\"}\n",
        )
        .expect("deterministic custom rule");
        let output = std::process::Command::new(std::env::current_exe().expect("unit test binary"))
            .arg("--exact")
            .arg(format!(
                "{}::{name}",
                module_path!().split_once("::").unwrap().1
            ))
            .arg("--nocapture")
            .env("CODESCRIBE_DATA_DIR", directory.path())
            .output()
            .expect("isolated lexicon test process");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "child test failed: {stdout}\n{stderr}"
        );
        assert!(
            stdout.contains("running 1 test\n"),
            "exact test must exist: {stdout}"
        );
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "no skipped execution: {stdout}"
        );
    }

    /// Positive configured rewrite through the real PhraseFinal consumer.
    /// Light+ capitalization/periods belong to PresentationEmitter, so they
    /// are deliberately absent here; raw Apple evidence remains unchanged.
    #[test]
    fn apple_seal_lexicon_corrects_sealed_final() {
        with_lexicon_fixture("apple_seal_lexicon_corrects_sealed_final", || {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut state = state("lexicon-phrase", 1.0);
            let occurrence = qualify(&mut state, 0.0, 1.0);
            emit(
                &mut state,
                &tx,
                vec![segment("uruchom doker teraz", 0.0, 1.0)],
            );
            let events = drain(&mut rx);
            assert_eq!(raw_finals(&events), vec!["uruchom doker teraz"]);
            assert_eq!(document(&state), "uruchom Docker teraz");
            assert_eq!(state.sealed_count, 1);
            let observations = events
                .iter()
                .filter_map(|event| match event {
                    EngineEvent::LedgerMutation {
                        observation, label, ..
                    } => {
                        assert_eq!(observation.occurrence, occurrence);
                        Some((observation.producer, label.as_str()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                observations,
                vec![
                    (LedgerObservationProducer::Apple, "uruchom doker teraz"),
                    (LedgerObservationProducer::Lexicon, "uruchom Docker teraz"),
                ]
            );
            assert!(events.iter().any(|event| matches!(event,
                EngineEvent::UtteranceFinal { text, .. } if text == "uruchom Docker teraz"
            )));
            assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&occurrence));
        });
    }

    #[test]
    fn apple_seal_lexicon_leaves_previews_raw() {
        with_lexicon_fixture("apple_seal_lexicon_leaves_previews_raw", || {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut state = state("raw-lexicon-preview", 1.0);
            emit_stream_events(
                vec![LiveStreamEvent::Partial {
                    text: "uruchom doker".into(),
                    segments: vec![segment("uruchom doker", 0.0, 1.0)],
                }],
                &tx,
                &mut state,
                1.0,
            );
            assert!(
                matches!(rx.try_recv().unwrap(), EngineEvent::Preview { text, .. } if text == "uruchom doker")
            );
            assert!(rx.try_recv().is_err());
            assert!(document(&state).is_empty());
            assert_eq!(state.sealed_count, 0);
        });
    }

    /// The old `:D` string filter has no live worker owner. Blank observations
    /// still cannot emit blank finals; a nonempty acoustically qualified label
    /// must not disappear merely because its spelling resembles an artifact.
    #[test]
    fn apple_seal_lexicon_empty_text_never_emits_a_blank_final() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("blank-final", 1.0, &[(0.0, 1.0)]);
        emit(&mut state, &tx, vec![segment("   ", 0.0, 1.0)]);
        assert!(document(&state).is_empty());
        assert_eq!(state.sealed_count, 0);
        assert!(raw_finals(&drain(&mut rx)).is_empty());
        // Independent positive control: text classification must not substitute
        // for evidence. A blank callback has already reconciled its own slice.
        let mut state = physical_state("spoken-symbol", 1.0, &[(0.0, 1.0)]);
        emit(&mut state, &tx, vec![segment(":D", 0.0, 1.0)]);
        assert_eq!(document(&state), ":D");
        assert_eq!(raw_finals(&drain(&mut rx)), vec![":D"]);
        assert_eq!(state.sealed_count, 1);
    }

    /// Positive configured rewrite through the real summary fallback. A later
    /// summary cannot bypass the occurrence owner to publish the document twice.
    #[test]
    fn apple_seal_lexicon_corrects_summary_fallback_seal() {
        with_lexicon_fixture("apple_seal_lexicon_corrects_summary_fallback_seal", || {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut state = state("summary-observers", 2.0);
            let occurrence = qualify(&mut state, 0.0, 2.0);
            let summary = LiveStreamEvent::Summary {
                text: "zbuduj obraz doker".into(),
                segments: vec![segment("zbuduj obraz doker", 0.0, 2.0)],
                ok: true,
                error: None,
            };
            emit_stream_events(vec![summary.clone()], &tx, &mut state, 2.0);
            let events = drain(&mut rx);
            let producers = events
                .iter()
                .filter_map(|event| match event {
                    EngineEvent::LedgerMutation {
                        observation, label, ..
                    } => {
                        assert_eq!(observation.occurrence, occurrence);
                        Some((observation.producer, label.as_str()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                producers,
                vec![
                    (LedgerObservationProducer::Apple, "zbuduj obraz doker"),
                    (LedgerObservationProducer::Lexicon, "zbuduj obraz Docker"),
                ]
            );
            assert_eq!(document(&state), "zbuduj obraz Docker");
            assert_eq!(raw_finals(&events), vec!["zbuduj obraz doker"]);
            assert!(state.acoustic_ledger.lock().unwrap().is_sealed(&occurrence));
            assert!(state.open_partial.is_empty());
            emit_stream_events(vec![summary], &tx, &mut state, 2.0);
            assert!(drain(&mut rx).is_empty());
            assert_eq!(state.sealed_count, 1);
        });
    }

    #[test]
    fn apple_seal_preserves_observed_text_until_ledger_repair() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("apple-raw", 1.0, &[(0.0, 1.0)]);
        emit(
            &mut state,
            &tx,
            vec![segment("uruchom doker teraz", 0.0, 1.0)],
        );
        assert_eq!(document(&state), "uruchom doker teraz");
        assert_eq!(raw_finals(&drain(&mut rx)), vec!["uruchom doker teraz"]);
        let occurrence = OccurrenceIdentity::new("apple-raw", 7, 0, sample(1.0));
        let late = admit_ledger_label(
            &mut state,
            &tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    99,
                    0,
                    occurrence,
                ),
                label: "uruchom Docker teraz",
                energy: EnergyAdmission::RequireExistingQualification,
            },
        )
        .unwrap();
        assert!(matches!(
            late,
            MutationReceipt::Refuse {
                reason: RefuseReason::SealedReplay,
                ..
            }
        ));
        assert_eq!(document(&state), "uruchom doker teraz");
    }

    #[test]
    fn apple_tail_patch_seal_enqueues_audio_window_for_the_sealed_utterance() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel(TAIL_PATCH_QUEUE_CAP);
        let mut state = physical_state("exact-tail", 6.0, &[(0.5, 2.0)]);
        state.fusion_context = FusionContextMode::SymmetricPad;
        state.tail_patch = Some(tail_tx);
        emit(&mut state, &tx, vec![segment("uruchom doker", 0.5, 2.0)]);
        assert!(
            raw_finals(&drain(&mut rx)).is_empty(),
            "Whisper still owns an open frontier"
        );
        assert!(state.flush_layer1_coalesce(&tx));
        let request = tail_rx.try_recv().expect("owned PCM request");
        assert_eq!(request.utterance_id, 1);
        assert_eq!(
            request.committed_text, "uruchom doker",
            "sliced lane preserves raw observed baseline"
        );
        assert_eq!(
            request.provider_request.identity.range.sample_start,
            sample(0.1)
        );
        assert_eq!(
            request.provider_request.identity.range.sample_end,
            sample(2.4)
        );
        assert_eq!(request.audio, vec![0.25; sample(2.3) as usize]);
        assert_eq!(
            request.provider_request.identity.request_id,
            request.utterance_id
        );
        assert_eq!(
            request.member_occurrences,
            vec![(
                1,
                OccurrenceIdentity::new("exact-tail", 7, sample(0.5), sample(2.0))
            )]
        );
        assert_eq!(state.tail_patch_awaiting_completion, 1);
        state.complete_whisper_window(
            &tx,
            TailPatchCompletion {
                utterance_id: request.utterance_id,
                request_identity: Some(request.provider_request.identity),
                member_occurrences: request.member_occurrences,
                payload: None,
            },
            6.0,
        );
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert_eq!(state.tail_patch_jobs_skipped, 1);
        assert_eq!(raw_finals(&drain(&mut rx)), vec!["uruchom doker"]);
    }

    #[test]
    fn seal_window_clamps_start_after_retention_eviction() {
        let mut state = state("evicted-head", 200.0);
        let retained_start = state.audio.retained_start_secs();
        assert!(retained_start > 0.0);
        let window = resolve_sealed_audio_window(&mut state, 150.0).expect("retained window");
        assert_eq!(
            window.sample_start,
            (f64::from(retained_start) * f64::from(RATE)) as u64
        );
        assert_eq!(window.sample_end, sample(150.0));
        let next = resolve_sealed_audio_window(&mut state, 180.0).expect("next retained window");
        assert_eq!(next.sample_start, sample(150.0));
        assert_eq!(next.sample_end, sample(180.0));
        assert!(resolve_sealed_audio_window(&mut state, 100.0).is_none());
        assert_eq!(state.unresolved_windows, 1);
    }

    #[test]
    fn apple_tail_patch_unresolved_window_enqueues_nothing() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel(TAIL_PATCH_QUEUE_CAP);
        let mut state = state("unresolved-tail", 2.0);
        state.tail_patch = Some(tail_tx);
        emit(&mut state, &tx, vec![segment("future phrase", 8.0, 9.0)]);
        assert!(!state.flush_layer1_coalesce(&tx));
        assert!(tail_rx.try_recv().is_err());
        assert_eq!(state.unresolved_windows, 1);
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert_eq!(state.sealed_count, 0, "unqualified text cannot seal");
        assert!(document(&state).is_empty());
        assert!(raw_finals(&drain(&mut rx)).is_empty());
    }

    #[test]
    fn apple_tail_patch_off_by_default_enqueues_no_jobs() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("tail-off", 4.0, &[(0.5, 2.0)]);
        assert!(
            state.tail_patch.is_none(),
            "arming is injected by session owner"
        );
        emit(&mut state, &tx, vec![segment("uruchom doker", 0.5, 2.0)]);
        assert!(!state.flush_layer1_coalesce(&tx));
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert_eq!(state.tail_patch_backpressure_drops, 0);
        assert_eq!(state.sealed_count, 1);
        assert_eq!(raw_finals(&drain(&mut rx)), vec!["uruchom doker"]);
    }

    #[test]
    fn apple_tail_patch_backpressure_retains_until_capacity_without_stalling_capture() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (tail_tx, mut tail_rx) = mpsc::channel(1);
        let ranges = (0..10)
            .map(|i| (i as f32 * 0.5, i as f32 * 0.5 + 0.4))
            .collect::<Vec<_>>();
        let mut state = physical_state("full-tail", 10.0, &ranges);
        state.tail_patch = Some(tail_tx);
        for (i, &(start, end)) in ranges.iter().enumerate() {
            emit(
                &mut state,
                &tx,
                vec![segment(&format!("segment {i}"), start, end)],
            );
        }
        state.flush_layer1_coalesce(&tx);
        assert_eq!(state.tail_patch_awaiting_completion, 1);
        // Ten physical occurrences remain conserved: one in transport, eight
        // retained exact jobs, and one explicit capacity failure. The old
        // nine-drop policy is replaced, not excused with a weaker assertion.
        assert_eq!(state.refinement_pending.len(), 8);
        assert_eq!(state.tail_patch_backpressure_drops, 1);
        assert_eq!(
            state.sealed_count, 0,
            "capacity failure returns the job, not a successful speech recovery"
        );
        let mut submitted = BTreeSet::new();
        for _ in 0..9 {
            let request = tail_rx.try_recv().expect("retained exact request");
            assert!(submitted.insert(request.utterance_id), "never submit twice");
            assert!(tail_rx.try_recv().is_err());
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            state.complete_whisper_window(
                &tx,
                TailPatchCompletion {
                    utterance_id: request.utterance_id,
                    request_identity: Some(request.provider_request.identity),
                    member_occurrences: request.member_occurrences,
                    payload: None,
                },
                10.0,
            );
            state.retry_refinements(&tx);
        }
        assert_eq!(submitted.len(), 9);
        assert!(state.refinement_pending.is_empty());
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert_eq!(state.sealed_count, 1);
        assert_eq!(raw_finals(&drain(&mut rx)).len(), 1);
        {
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.pending_text_recoveries("full-tail", 7).len(), 9);
            for (i, &(start, end)) in ranges.iter().enumerate() {
                let occurrence =
                    OccurrenceIdentity::new("full-tail", 7, sample(start), sample(end));
                assert_eq!(
                    ledger.text_of(&occurrence),
                    Some(format!("segment {i}").as_str()),
                    "all physical labels remain visible even when recovery failed"
                );
            }
        }
        let before = state.audio.session_sample_end();
        state.audio.push(&[0.25; 512]);
        assert_eq!(state.audio.session_sample_end(), before + 512);
    }

    #[test]
    fn fusion_slices_admit_disjoint_ledger_occurrences_before_raw_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("slice-order", 4.0, &[(0.0, 1.0), (2.0, 3.0)]);
        emit(
            &mut state,
            &tx,
            vec![segment("Iwo", 0.0, 1.0), segment("Iwo", 2.0, 3.0)],
        );
        let events = drain(&mut rx);
        assert_eq!(raw_finals(&events), vec!["Iwo", "Iwo"]);
        for (id, start, end) in [(1, 0.0, 1.0), (2, 2.0, 3.0)] {
            let occurrence = OccurrenceIdentity::new("slice-order", 7, sample(start), sample(end));
            let admission = events.iter().position(|event| matches!(event,
                EngineEvent::LedgerMutation { observation, receipt, label }
                    if observation.producer == LedgerObservationProducer::Apple
                        && observation.occurrence == occurrence && receipt.grants_mutation() && label == "Iwo"
            )).expect("accepted slice-local Apple observation");
            let seal = events
                .iter()
                .position(|event| {
                    matches!(event,
                        EngineEvent::LedgerSeal { receipt } if receipt.coverage == occurrence
                    )
                })
                .expect("same occurrence seal");
            let final_index = events
                .iter()
                .position(|event| {
                    matches!(event,
                        EngineEvent::UtteranceFinal { utterance_id, .. } if *utterance_id == id
                    )
                })
                .expect("raw final");
            assert!(admission < seal && seal < final_index);
        }
        assert_eq!(document(&state), "Iwo Iwo");
    }

    /// Sliced replay is stopped by reconciled physical identity before a new
    /// admission. Also exercise the ledger's independent replay fence directly.
    #[test]
    fn fusion_slice_replay_reaches_ledger_identity_refusal() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("slice-replay", 4.0, &[(0.0, 1.0), (2.0, 3.0)]);
        let words = vec![segment("Iwo", 0.0, 1.0), segment("Iwo", 2.0, 3.0)];
        emit(&mut state, &tx, words.clone());
        drain(&mut rx);
        emit(&mut state, &tx, words);
        assert!(
            drain(&mut rx).is_empty(),
            "reconciled slice must not re-emit events"
        );
        for (id, start, end) in [(1, 0.0, 1.0), (2, 2.0, 3.0)] {
            let receipt = admit_ledger_label(
                &mut state,
                &tx,
                LabelAdmission {
                    observation: LedgerObservationIdentity::new(
                        LedgerObservationProducer::Apple,
                        id,
                        0,
                        OccurrenceIdentity::new("slice-replay", 7, sample(start), sample(end)),
                    ),
                    label: "changed replay",
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .unwrap();
            assert!(!receipt.grants_mutation());
        }
        assert_eq!(document(&state), "Iwo Iwo");
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().occurrences().count(),
            2
        );
    }

    #[test]
    fn five_disjoint_iwo_segments_all_reach_the_final() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("five-words", 3.0, &[(0.0, 2.0)]);
        emit(
            &mut state,
            &tx,
            (0..5)
                .map(|i| {
                    let start = i as f32 * 0.4;
                    segment("Iwo", start, start + 0.3)
                })
                .collect(),
        );
        let events = drain(&mut rx);
        let finals = raw_finals(&events);
        assert_eq!(
            finals.len(),
            1,
            "one physical occurrence contains five words"
        );
        assert_eq!(count_iwo(finals[0]), 5);
        assert_eq!(count_iwo(&document(&state)), 5);
    }

    #[test]
    fn cumulative_fifth_iwo_is_not_absorbed_as_a_revision() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = physical_state("fifth-iwo", 3.0, &[(0.0, 1.5), (1.6, 1.9)]);
        let first = (0..4)
            .map(|i| {
                let start = i as f32 * 0.4;
                segment("Iwo", start, start + 0.3)
            })
            .collect::<Vec<_>>();
        emit(&mut state, &tx, first.clone());
        assert_eq!(count_iwo(&document(&state)), 4);
        let mut cumulative = first;
        cumulative.push(segment("Iwo", 1.6, 1.9));
        emit(&mut state, &tx, cumulative);
        assert_eq!(count_iwo(&document(&state)), 5);
        assert_eq!(
            raw_finals(&drain(&mut rx))
                .iter()
                .map(|text| count_iwo(text))
                .sum::<usize>(),
            5
        );
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().occurrences().count(),
            2
        );
    }
}

/// rc-w2-composer-turn: the live paid-formatter arming seam.
///
/// Deliberately its own module. The file's main `mod tests` carries a large
/// parked surface; a falsifier for a new production owner must not inherit that
/// state, and must not be silently disabled with it.
#[cfg(test)]
mod composer_turn_formatter_arming_tests {
    use super::{CaptureTurnIntent, FormattingPolicy, live_formatter_lane_is_armed};

    /// Every configuration that would arm the live lane for a hands-free take.
    const FULLY_ARMED: (bool, FormattingPolicy, bool) = (true, FormattingPolicy::Correction, true);

    #[test]
    fn unarmed_live_formatting_never_resolves_credentials() {
        for (intent, enabled, policy) in [
            (
                CaptureTurnIntent::SingleTurn,
                true,
                FormattingPolicy::Correction,
            ),
            (
                CaptureTurnIntent::HandsFree,
                false,
                FormattingPolicy::Correction,
            ),
            (CaptureTurnIntent::HandsFree, true, FormattingPolicy::Off),
            (CaptureTurnIntent::HandsFree, true, FormattingPolicy::Max),
            (CaptureTurnIntent::SingleTurn, true, FormattingPolicy::Max),
        ] {
            assert!(!live_formatter_lane_is_armed(
                intent,
                enabled,
                policy,
                || { panic!("an unarmed live lane must not enter the credential-use boundary") }
            ));
        }
    }

    #[test]
    fn a_one_turn_take_never_arms_the_live_formatter_lane() {
        let (enabled, policy, available) = FULLY_ARMED;
        assert!(
            !live_formatter_lane_is_armed(CaptureTurnIntent::SingleTurn, enabled, policy, || {
                available
            }),
            "a composer turn must not open paid provider slots per sealed fragment, \
             even with formatting fully enabled, a non-Off policy and a live lane"
        );
    }

    #[test]
    fn a_hands_free_take_keeps_its_existing_live_lane() {
        let (enabled, policy, available) = FULLY_ARMED;
        assert!(
            live_formatter_lane_is_armed(CaptureTurnIntent::HandsFree, enabled, policy, || {
                available
            }),
            "hotkey, tray and overlay takes keep the per-occurrence formatter they had"
        );
    }

    /// The intent is an additional refusal, never a way to arm a lane the
    /// configuration itself refuses.
    #[test]
    fn hands_free_cannot_arm_a_lane_the_configuration_refuses() {
        for (enabled, policy, available) in [
            (false, FormattingPolicy::Correction, true),
            (true, FormattingPolicy::Off, true),
            (true, FormattingPolicy::Correction, false),
        ] {
            assert!(
                !live_formatter_lane_is_armed(
                    CaptureTurnIntent::HandsFree,
                    enabled,
                    policy,
                    || available
                ),
                "capture intent must not override formatter configuration or transport ownership"
            );
        }
    }
}

/// W2 contracts: virtual time, owned synthetic PCM, no audio/model invocation.
#[cfg(test)]
mod live_refinement_admission_tests {
    use super::super::silero_fusion::UtteranceLedger;
    use super::*;

    const RATE: u32 = 1_000;

    fn fixture(
        capacity: usize,
    ) -> (
        AppleSealState,
        mpsc::UnboundedSender<EngineEvent>,
        mpsc::UnboundedReceiver<EngineEvent>,
        mpsc::Receiver<TailPatchRequest>,
    ) {
        let (events, receiver) = mpsc::unbounded_channel();
        let (sender, requests) = mpsc::channel(capacity);
        let mut state = AppleSealState::new_with_tail_patch_for_session(
            RATE,
            "live-admission".into(),
            7,
            sender,
            Arc::new(Mutex::new(AcousticLedger::new())),
            Some(EnergyCalibration {
                version: "owned-fixture".into(),
                min_energy_integral: 1.0,
                min_valley_samples: 1,
            }),
        );
        state.audio.push(&vec![0.25; 20_000]);
        (state, events, receiver, requests)
    }

    fn closed(count: u64) -> UtteranceLedger {
        let mut ledger = UtteranceLedger::new();
        for id in 0..count {
            ledger.open_or_extend("live-admission", 7, id * 1_000, id * 1_000 + 400);
            ledger.close_open(id * 1_000 + 400);
        }
        ledger
    }

    fn finish(request: &TailPatchRequest) -> TailPatchCompletion {
        TailPatchCompletion {
            utterance_id: request.utterance_id,
            request_identity: Some(request.provider_request.identity.clone()),
            payload: None,
            member_occurrences: request.member_occurrences.clone(),
        }
    }

    fn warnings(receiver: &mut mpsc::UnboundedReceiver<EngineEvent>, code: &str) -> usize {
        std::iter::from_fn(|| receiver.try_recv().ok())
            .filter(
                |event| matches!(event, EngineEvent::Warning { code: found, .. } if found == code),
            )
            .count()
    }

    fn labelled_completion(request: &TailPatchRequest) -> TailPatchCompletion {
        use crate::stt::tail_provider::{
            TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
            TailTimingQuality,
        };
        let occurrence = &request.member_occurrences[0].1;
        let mut completion = finish(request);
        completion.payload = Some(TailProviderPayload {
            identity: request.provider_request.identity.clone(),
            text: "hello".into(),
            segments: vec![TimedTailSegment {
                grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                text: "hello".into(),
                range: TailSampleRange {
                    session: occurrence.session.clone(),
                    capture_epoch: occurrence.capture_epoch,
                    sample_start: occurrence.sample_start,
                    sample_end: occurrence.sample_end,
                },
            }],
            avg_logprob: None,
            compression_ratio: None,
            provider_id: TailProviderId::Fake,
            elapsed_ms: 0,
            evidence: TailProviderEvidence {
                segment_grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                source: TailEvidenceSource::Whisper,
                revision: Some("synthetic-live-admission".into()),
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: None,
            },
        });
        completion
    }

    /// Synthetic boundary-driven integration witness, not a microphone proof.
    /// Exercise production reconciliation and admission with partial Apple text.
    #[test]
    fn synthetic_silero_guardian_recovers_whole_occurrence_or_keeps_explicit_debt() {
        use super::super::silero_fusion::SileroIngress;
        use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
        for (succeeds, resumes) in [(false, false), (true, false), (false, true), (true, true)] {
            let (mut state, events, mut receiver, mut requests) = fixture(1);
            let mut fusion = SileroIngress::new(RATE, "live-admission", 7);
            fusion.note_observed_pcm(200, 200);
            fusion.observe_boundaries(&[VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 0,
                speech_probability: 0.95,
            }]);
            state
                .speech_progress
                .observe_speech(&fusion.acoustic_speech_evidence(), true, RATE);
            state.speech_progress.observe_apple("partial words", 200);
            fusion.note_observed_pcm(2_400, 2_600);
            fusion.observe_boundaries(&[VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 2_600,
                speech_probability: 0.05,
            }]);
            let speech = fusion.acoustic_speech_evidence();
            state.speech_progress.observe_speech(&speech, false, RATE);
            assert_eq!(state.speech_progress.debt_ms(RATE), 2_400);
            assert_eq!(
                state.speech_progress.phase(RATE),
                crate::pipeline::contracts::SpeechIntegrityPhase::Stalled
            );
            if resumes {
                state
                    .speech_progress
                    .observe_apple("partial words and latest words", 2_600);
                assert_eq!(state.speech_progress.debt_ms(RATE), 0);
            }
            state.fusion = Some(fusion);
            let mut physical = UtteranceLedger::new();
            physical.open_or_extend("live-admission", 7, 0, 2_600);
            physical.close_open(2_600);
            reconcile_silero_ledger(
                &mut state,
                &events,
                &physical,
                &[TranscriptSegment {
                    text: "partial words".into(),
                    start_ts: 0.0,
                    end_ts: 0.2,
                }],
            );
            let request = requests.try_recv().expect("debt submits at speech close");
            let occurrence = OccurrenceIdentity::new("live-admission", 7, 0, 2_600);
            assert_eq!(request.member_occurrences, vec![(1, occurrence.clone())]);
            assert_eq!(
                OccurrenceIdentity::from(&request.provider_request.identity.range),
                occurrence
            );
            assert_eq!(request.audio, vec![0.25; 2_600]);
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            assert!(state.layer1_coalesce.is_empty());
            {
                let ledger = state.acoustic_ledger.lock().unwrap();
                assert_eq!(ledger.text_of(&occurrence), Some("partial words"));
                assert!(ledger.text_recovery_pending(&occurrence));
                assert_eq!(
                    ledger
                        .assess_seal_coverage("live-admission", 7, &speech, 250)
                        .coverage_ratio(),
                    Some(0.0)
                );
            }
            while receiver.try_recv().is_ok() {}
            let mut completion = if succeeds {
                labelled_completion(&request)
            } else {
                finish(&request)
            };
            if let Some(payload) = completion.payload.as_mut() {
                payload.text = "partial words and the recovered remainder".into();
                payload.segments[0].text = payload.text.clone();
            }
            state.complete_whisper_window(&events, completion, 20.0);
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_recovery_pending(&occurrence), !succeeds);
            assert_eq!(ledger.is_sealed(&occurrence), succeeds);
            assert_eq!(
                ledger.text_of(&occurrence),
                Some(if succeeds {
                    "partial words and the recovered remainder"
                } else {
                    "partial words"
                })
            );
            let coverage = ledger.assess_seal_coverage("live-admission", 7, &speech, 250);
            assert_eq!(
                coverage.coverage_ratio(),
                Some(if succeeds { 1.0 } else { 0.0 })
            );
            assert!(ledger.record_seal_coverage(coverage));
            if succeeds {
                assert!(ledger.seal_terminal("live-admission", 7).is_ok());
            } else {
                assert_eq!(
                    ledger.seal_terminal("live-admission", 7),
                    Err(SealRefusal::TextRecoveryPending)
                );
            }
            drop(ledger);
            assert_eq!(
                state
                    .speech_progress
                    .occurrence_has_debt(&occurrence, &speech, RATE),
                !succeeds,
            );
            let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
            assert_eq!(emitted.iter().any(|event| matches!(event,
                EngineEvent::LedgerMutation { observation, receipt: MutationReceipt::Correct { .. }, label }
                    if observation.producer == LedgerObservationProducer::Whisper
                    && label == "partial words and the recovered remainder"
            )), succeeds);
            assert_eq!(
                emitted
                    .iter()
                    .any(|event| matches!(event, EngineEvent::LedgerSeal { .. })),
                succeeds
            );
            assert!(requests.try_recv().is_err());
        }
    }

    #[test]
    fn whisper_segmentless_text_requires_exact_window_and_rejected_pins_never_fallback() {
        for context in [
            FusionContextMode::UtteranceOnly,
            FusionContextMode::SymmetricPad,
        ] {
            for defect in ["none", "session", "epoch", "outside", "empty", "reversed"] {
                let (mut state, events, mut receiver, mut requests) = fixture(1);
                state.fusion_context = context;
                reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
                assert!(state.layer1_coalesce.is_empty());
                let request = requests.try_recv().unwrap();
                request
                    .provider_request
                    .validate_pcm(&request.audio)
                    .unwrap();
                let occurrence = &request.member_occurrences[0].1;
                let mut completion = labelled_completion(&request);
                let payload = completion.payload.as_mut().unwrap();
                match defect {
                    "none" => payload.segments.clear(),
                    "session" => payload.segments[0].range.session = "foreign".into(),
                    "epoch" => payload.segments[0].range.capture_epoch += 1,
                    "outside" => payload.segments[0].range.sample_end += 1,
                    "empty" => payload.segments[0].range.sample_end = 0,
                    "reversed" => payload.segments[0].range.sample_start = 401,
                    _ => unreachable!(),
                }
                while receiver.try_recv().is_ok() {}
                state.complete_whisper_window(&events, completion, 20.0);
                let accepted = defect == "none";
                assert_eq!(
                    OccurrenceIdentity::from(&request.provider_request.identity.range),
                    *occurrence
                );
                let ledger = state.acoustic_ledger.lock().unwrap();
                assert_eq!(
                    ledger.text_of(occurrence),
                    accepted.then_some("hello"),
                    "{context:?}/{defect}"
                );
                assert_eq!(ledger.is_sealed(occurrence), accepted);
                assert!(
                    ledger
                        .frontier_of(occurrence)
                        .unwrap()
                        .open_producers()
                        .is_empty()
                );
                drop(ledger);
                assert_eq!(state.tail_patch_awaiting_completion, 0);
                assert!(state.refinement_submitted.is_empty());
                let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
                assert_eq!(
                    emitted
                        .iter()
                        .filter(|event| matches!(event,
                            EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
                        ))
                        .count(),
                    usize::from(accepted)
                );
                assert_eq!(
                    emitted
                        .iter()
                        .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                        .count(),
                    usize::from(accepted)
                );
                // A corrected payload on a consumed request is still a replay.
                state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
                assert!(receiver.try_recv().is_err());
                assert_eq!(
                    state.acoustic_ledger.lock().unwrap().text_of(occurrence),
                    accepted.then_some("hello")
                );
            }
        }
    }

    #[test]
    fn whisper_foreign_envelopes_preserve_the_submitted_job_until_exact_completion() {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
        assert!(state.layer1_coalesce.is_empty());
        let request = requests.try_recv().unwrap();
        let occurrence = &request.member_occurrences[0].1;
        while receiver.try_recv().is_ok() {}
        for defect in [
            "request",
            "session",
            "epoch",
            "start",
            "end",
            "member",
            "member_span",
            "members",
            "utterance",
            "missing",
        ] {
            let mut completion = labelled_completion(&request);
            let identity = completion.request_identity.as_mut().unwrap();
            match defect {
                "request" => identity.request_id += 1,
                "session" => identity.range.session = "foreign".into(),
                "epoch" => identity.range.capture_epoch += 1,
                "start" => identity.range.sample_start += 1,
                "end" => identity.range.sample_end += 1,
                "member" => completion.member_occurrences[0].0 += 1,
                "member_span" => completion.member_occurrences[0].1.sample_end += 1,
                "members" => completion.member_occurrences.clear(),
                "utterance" => completion.utterance_id += 1,
                "missing" => completion.request_identity = None,
                _ => unreachable!(),
            }
            state.complete_whisper_window(&events, completion, 20.0);
            assert_eq!(state.tail_patch_awaiting_completion, 1, "{defect}");
            assert_eq!(state.refinement_submitted.len(), 1);
            assert!(state.pending_events.contains_key(&request.utterance_id));
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(occurrence), None);
            assert!(!ledger.is_sealed(occurrence));
            assert!(
                ledger
                    .frontier_of(occurrence)
                    .unwrap()
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper)
            );
            drop(ledger);
            assert!(receiver.try_recv().is_err());
        }
        state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().text_of(occurrence),
            Some("hello")
        );
        assert!(state.acoustic_ledger.lock().unwrap().is_sealed(occurrence));
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert!(state.refinement_submitted.is_empty());
        let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            emitted
                .iter()
                .filter(|event| matches!(event,
                    EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
                ))
                .count(),
            1
        );
        assert_eq!(
            emitted
                .iter()
                .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                .count(),
            1
        );
        state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn whisper_unsolicited_completion_cannot_consume_a_held_occurrence() {
        let (mut source, source_events, _source_receiver, mut source_requests) = fixture(1);
        reconcile_silero_ledger(&mut source, &source_events, &closed(1), &[]);
        assert!(source.layer1_coalesce.is_empty());
        let unsolicited = source_requests.try_recv().unwrap();
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        // A labelled, debt-free occurrence still exercises held coalescer
        // ownership; a blank occurrence is now submitted immediately.
        reconcile_silero_ledger(
            &mut state,
            &events,
            &closed(1),
            &[TranscriptSegment {
                text: "hello".into(),
                start_ts: 0.0,
                end_ts: 0.4,
            }],
        );
        let occurrence = &unsolicited.member_occurrences[0].1;
        while receiver.try_recv().is_ok() {}
        state.complete_whisper_window(&events, labelled_completion(&unsolicited), 20.0);
        assert!(receiver.try_recv().is_err());
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert!(state.refinement_submitted.is_empty());
        assert!(!state.layer1_coalesce.is_empty());
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(occurrence), Some("hello"));
        assert!(!ledger.is_sealed(occurrence));
        assert!(
            ledger
                .frontier_of(occurrence)
                .unwrap()
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        );
        drop(ledger);
        assert!(state.flush_layer1_coalesce(&events));
        let request = requests.try_recv().unwrap();
        state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().text_of(occurrence),
            Some("hello")
        );
        assert!(state.acoustic_ledger.lock().unwrap().is_sealed(occurrence));
    }

    #[test]
    fn whisper_foreign_payload_returns_only_the_owned_frontier_without_text() {
        for defect in ["request", "session", "epoch", "start", "end"] {
            let (mut state, events, mut receiver, mut requests) = fixture(1);
            reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
            assert!(state.layer1_coalesce.is_empty());
            let request = requests.try_recv().unwrap();
            let occurrence = &request.member_occurrences[0].1;
            let mut completion = labelled_completion(&request);
            let identity = &mut completion.payload.as_mut().unwrap().identity;
            match defect {
                "request" => identity.request_id += 1,
                "session" => identity.range.session = "foreign".into(),
                "epoch" => identity.range.capture_epoch += 1,
                "start" => identity.range.sample_start += 1,
                "end" => identity.range.sample_end += 1,
                _ => unreachable!(),
            }
            while receiver.try_recv().is_ok() {}
            state.complete_whisper_window(&events, completion, 20.0);
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(occurrence), None, "{defect}");
            assert!(!ledger.is_sealed(occurrence));
            assert!(
                ledger
                    .frontier_of(occurrence)
                    .unwrap()
                    .open_producers()
                    .is_empty()
            );
            drop(ledger);
            assert_eq!(state.tail_patch_awaiting_completion, 0);
            assert!(state.refinement_submitted.is_empty());
            let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
            assert!(!emitted.iter().any(|event| matches!(event,
                EngineEvent::LedgerMutation { receipt, .. } if receipt.grants_mutation()
            )));
            assert!(
                !emitted
                    .iter()
                    .any(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
            );
            assert!(emitted.iter().any(|event| matches!(event,
                EngineEvent::Warning { code, .. } if code == TAIL_PATCH_IDENTITY_MISMATCH_WARNING_CODE
            )));
            state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn closed_without_next_apple_piece_submits_immediately_in_both_capture_intents() {
        // The production tick has no intent input: neither SingleTurn nor
        // hands-free lifecycle may veto acoustic readiness.
        for silence in [None, Some(2.0)] {
            let (mut state, events, _receiver, mut requests) = fixture(1);
            let _epoch = EpochGate::for_session(RATE, silence, true);
            let now = state.refinement_clock;
            reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
            let request = requests
                .try_recv()
                .expect("speech debt submits at close, without another Apple piece or tick");
            state.tick_refinements(&events, now + Duration::from_millis(1_199));
            assert!(requests.try_recv().is_err());
            state.audio.push(&[0.0; 40]);
            state.tick_refinements(&events, now + Duration::from_millis(1_200));
            assert!(requests.try_recv().is_err());
            assert_eq!(
                request.member_occurrences,
                vec![(1, OccurrenceIdentity::new("live-admission", 7, 0, 400))]
            );
            request
                .provider_request
                .validate_pcm(&request.audio)
                .expect("exact PCM");
            assert!(
                request.committed_text.is_empty(),
                "never manufacture Apple words"
            );
            for quantum in 1..10 {
                state.tick_refinements(
                    &events,
                    now + Duration::from_millis(1_200) + LIVE_WORKER_QUANTUM * quantum,
                );
            }
            assert!(
                requests.try_recv().is_err(),
                "idle ticks cannot replay admission"
            );
            assert_eq!(state.refinement_submitted.len(), 1);
        }
    }

    #[test]
    fn blank_then_nonblank_apple_evidence_keeps_one_refinement_owner() {
        let (mut state, events, mut receiver, mut requests) = fixture(2);
        let ledger = closed(1);
        reconcile_silero_ledger(
            &mut state,
            &events,
            &ledger,
            &[TranscriptSegment {
                text: " ".into(),
                start_ts: 0.0,
                end_ts: 0.4,
            }],
        );
        assert!(!state.reconciled_silero.contains(&1));
        reconcile_silero_ledger(
            &mut state,
            &events,
            &ledger,
            &[TranscriptSegment {
                text: "hello".into(),
                start_ts: 0.0,
                end_ts: 0.4,
            }],
        );
        assert!(state.reconciled_silero.contains(&1));
        state.flush_layer1_coalesce(&events);
        let request = requests.try_recv().expect("one physical job");
        assert!(requests.try_recv().is_err());
        assert_eq!(request.member_occurrences.len(), 1);
        let occurrence = &request.member_occurrences[0].1;
        assert_eq!(
            state.acoustic_ledger.lock().unwrap().text_of(occurrence),
            Some("hello")
        );
        state.complete_whisper_window(&events, labelled_completion(&request), 20.0);
        assert!(state.acoustic_ledger.lock().unwrap().is_sealed(occurrence));
        let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { label, receipt: MutationReceipt::Insert { .. }, .. }
                if label == "hello"
        )).count(), 1);
        assert!(!emitted.iter().any(|event| matches!(event,
            EngineEvent::LedgerMutation { label, receipt, .. }
                if label.trim().is_empty() && receipt.grants_mutation()
        )));
    }

    #[test]
    fn whisper_first_completion_admits_owned_label_without_apple_words() {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
        state.flush_layer1_coalesce(&events);
        let request = requests.try_recv().unwrap();
        let occurrence = request.member_occurrences[0].1.clone();
        assert_eq!(occurrence.sample_end, 400);
        assert_eq!(request.provider_request.identity.range.sample_end, 400);
        request
            .provider_request
            .validate_pcm(&request.audio)
            .unwrap();
        let completion = labelled_completion(&request);
        state.complete_whisper_window(&events, completion, 20.0);
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&occurrence), Some("hello"));
        assert!(ledger.is_sealed(&occurrence));
        assert_eq!(state.tail_patch_jobs_applied, 1);
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        let seal = ledger.seal_of(&occurrence).unwrap().clone();
        drop(ledger);
        let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert!(!emitted.iter().any(|event| matches!(event,
            EngineEvent::Warning { code, .. } if code == RefinementFailure::NoLabel.code())));
        assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, label, receipt: MutationReceipt::Insert { .. } }
                if observation.producer == LedgerObservationProducer::Whisper && label == "hello"
        )).count(), 1);
        assert_eq!(
            emitted
                .iter()
                .filter(|event| matches!(event,
                    EngineEvent::UtteranceFinal { text, .. } if text == "hello"
                ))
                .count(),
            1
        );
        reconcile_silero_ledger(
            &mut state,
            &events,
            &closed(1),
            &[TranscriptSegment {
                text: "late replacement".into(),
                start_ts: 0.0,
                end_ts: 0.4,
            }],
        );
        let ledger = state.acoustic_ledger.lock().unwrap();
        assert_eq!(ledger.text_of(&occurrence), Some("hello"));
        assert_eq!(ledger.seal_of(&occurrence), Some(&seal));
        assert_eq!(ledger.post_seal_decisions(&occurrence).len(), 1);
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn apple_first_emits_once_only_reducer_receipts_before_stop() {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        let words = [TranscriptSegment {
            text: "hello".into(),
            start_ts: 0.0,
            end_ts: 0.4,
        }];
        reconcile_silero_ledger(&mut state, &events, &closed(1), &words);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &words);
        state.flush_layer1_coalesce(&events);
        let request = requests.try_recv().unwrap();
        state.complete_whisper_window(&events, finish(&request), 20.0);
        state.complete_whisper_window(&events, finish(&request), 20.0);
        let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, label, receipt: MutationReceipt::Insert { .. } }
                if observation.producer == LedgerObservationProducer::Apple && label == "hello"
        )).count(), 1);
        assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, receipt: MutationReceipt::Preserve { .. }, .. }
                if observation.producer == LedgerObservationProducer::Lexicon
        )).count(), 1);
        assert_eq!(
            emitted
                .iter()
                .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                .count(),
            1
        );
        assert_eq!(state.sealed_count, 1);
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert!(requests.try_recv().is_err());
    }

    /// Observer completion without a label leaves the physical occurrence
    /// recoverable. Authored contract, deliberately unrun under W2.
    #[test]
    fn late_nonblank_apple_after_no_label_completion_remains_recoverable() {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        let physical = closed(1);
        reconcile_silero_ledger(&mut state, &events, &physical, &[]);
        state.flush_layer1_coalesce(&events);
        let request = requests.try_recv().unwrap();
        let occurrence = request.member_occurrences[0].1.clone();
        let serial = state
            .acoustic_ledger
            .lock()
            .unwrap()
            .serial_of(&occurrence)
            .unwrap()
            .clone();
        state.complete_whisper_window(&events, finish(&request), 20.0);
        state.complete_whisper_window(&events, finish(&request), 20.0);
        {
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger.seal(&occurrence),
                Err(SealRefusal::TextRecoveryPending)
            );
            assert!(ledger.frontier_of(&occurrence).unwrap().is_closed());
            assert_eq!(ledger.layer_trail_for(&occurrence).count(), 1);
        }
        assert_eq!(state.sealed_count, 0);
        reconcile_silero_ledger(
            &mut state,
            &events,
            &physical,
            &[TranscriptSegment {
                text: "late real words".into(),
                start_ts: 0.0,
                end_ts: 0.4,
            }],
        );
        {
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger.text_of(&occurrence),
                Some("late real words"),
                "a failed refinement must not consume the only later real label"
            );
            assert_eq!(ledger.serial_of(&occurrence), Some(&serial));
            assert_eq!(
                ledger.qualified_occurrences().cloned().collect::<Vec<_>>(),
                vec![occurrence.clone()]
            );
            assert!(ledger.seal_of(&occurrence).is_none());
            assert!(ledger.text_recovery_pending(&occurrence));
            assert_eq!(ledger.layer_trail_for(&occurrence).count(), 3);
            assert!(ledger.frontier_of(&occurrence).unwrap().is_closed());
        }
        let emitted = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, label, receipt: MutationReceipt::Insert { occurrence: held } }
                if observation.producer == LedgerObservationProducer::Apple
                    && observation.occurrence == occurrence && *held == occurrence
                    && label == "late real words"
        )).count(), 1);
        assert_eq!(emitted.iter().filter(|event| matches!(event,
            EngineEvent::LedgerMutation { observation, receipt: MutationReceipt::Preserve { .. }, .. }
                if observation.producer == LedgerObservationProducer::Lexicon
        )).count(), 1);
        assert_eq!(
            emitted
                .iter()
                .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. }))
                .count(),
            0
        );
        assert_eq!(
            emitted
                .iter()
                .filter(|event| matches!(event,
                    EngineEvent::UtteranceFinal { text, .. } if text == "late real words"
                ))
                .count(),
            0
        );
        let later = state.refinement_clock + Duration::from_secs(3);
        state.tick_refinements(&events, later);
        assert!(
            requests.try_recv().is_err(),
            "same occurrence never decodes twice"
        );
        assert_eq!(state.tail_patch_jobs_skipped, 1);
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert_eq!(
            state.window_by_samples(0, 400).unwrap().samples,
            vec![0.25; 400]
        );
    }

    #[test]
    fn late_apple_with_foreign_capture_cannot_qualify_or_replace_current_occurrence() {
        for (session, epoch) in [("other", 7), ("live-admission", 6)] {
            let (mut state, events, _receiver, mut requests) = fixture(1);
            reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
            state.flush_layer1_coalesce(&events);
            let request = requests.try_recv().unwrap();
            state.complete_whisper_window(&events, finish(&request), 20.0);
            let mut stale = UtteranceLedger::new();
            stale.open_or_extend(session, epoch, 0, 400);
            stale.close_open(400);
            reconcile_silero_ledger(
                &mut state,
                &events,
                &stale,
                &[TranscriptSegment {
                    text: "foreign".into(),
                    start_ts: 0.0,
                    end_ts: 0.4,
                }],
            );
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(
                ledger.qualified_occurrences().cloned().collect::<Vec<_>>(),
                vec![request.member_occurrences[0].1.clone()]
            );
            assert!(ledger.is_empty());
            drop(ledger);
            reconcile_silero_ledger(
                &mut state,
                &events,
                &closed(1),
                &[TranscriptSegment {
                    text: "current".into(),
                    start_ts: 0.0,
                    end_ts: 0.4,
                }],
            );
            assert_eq!(
                state
                    .acoustic_ledger
                    .lock()
                    .unwrap()
                    .text_of(&request.member_occurrences[0].1),
                Some("current")
            );
            assert_eq!(
                state.window_by_samples(0, 400).unwrap().samples,
                vec![0.25; 400]
            );
        }
    }

    #[test]
    fn oversized_owned_pcm_is_classified_without_an_unbounded_backlog() {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        state.audio.push(&vec![0.25; 20_000]);
        let mut ledger = UtteranceLedger::new();
        ledger.open_or_extend("live-admission", 7, 0, 40_000);
        ledger.close_open(40_000);
        reconcile_silero_ledger(&mut state, &events, &ledger, &[]);
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::BacklogExhausted.code()),
            1
        );
        assert!(state.refinement_pending.is_empty());
        assert!(state.layer1_coalesce.is_empty());
        assert!(requests.try_recv().is_err());
        assert_eq!(state.audio.session_sample_end(), 40_000);
    }

    #[test]
    fn saturation_then_drain_preserves_exact_requests_and_once_only_submission() {
        let (mut state, events, _receiver, mut requests) = fixture(1);
        reconcile_silero_ledger(&mut state, &events, &closed(3), &[]);
        state.flush_layer1_coalesce(&events);
        assert_eq!(state.refinement_pending.len(), 2);
        assert_eq!(state.tail_patch_backpressure_drops, 0);
        let mut ids = BTreeSet::new();
        for _ in 0..3 {
            let request = requests.try_recv().expect("next exact request");
            assert!(ids.insert(request.utterance_id));
            request
                .provider_request
                .validate_pcm(&request.audio)
                .unwrap();
            let completion = finish(&request);
            state.complete_whisper_window(&events, completion, 20.0);
            state.retry_refinements(&events);
        }
        for id in 0..3 {
            reconcile_silero_ledger(
                &mut state,
                &events,
                &closed(3),
                &[TranscriptSegment {
                    text: "recovered".into(),
                    start_ts: id as f32,
                    end_ts: id as f32 + 0.4,
                }],
            );
            let occurrence =
                OccurrenceIdentity::new("live-admission", 7, id * 1_000, id * 1_000 + 400);
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&occurrence), Some("recovered"));
            assert!(!ledger.is_sealed(&occurrence));
            assert!(ledger.text_recovery_pending(&occurrence));
            assert_eq!(ledger.layer_trail_for(&occurrence).count(), 3);
        }
        assert!(requests.try_recv().is_err());
        assert_eq!(ids, BTreeSet::from([1, 2, 3]));
        assert!(state.refinement_pending.is_empty());
        assert!(state.refinement_submitted.is_empty());
        assert_eq!(state.tail_patch_awaiting_completion, 0);
    }

    #[test]
    fn permanent_lane_loss_reports_every_pending_occurrence_once() {
        let (mut state, events, mut receiver, requests) = fixture(1);
        drop(requests);
        reconcile_silero_ledger(&mut state, &events, &closed(3), &[]);
        state.flush_layer1_coalesce(&events);
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::LaneGone.code()),
            3
        );
        state.retry_refinements(&events);
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::LaneGone.code()),
            0
        );
        for id in 0..3 {
            reconcile_silero_ledger(
                &mut state,
                &events,
                &closed(3),
                &[TranscriptSegment {
                    text: "recovered".into(),
                    start_ts: id as f32,
                    end_ts: id as f32 + 0.4,
                }],
            );
            let occurrence =
                OccurrenceIdentity::new("live-admission", 7, id * 1_000, id * 1_000 + 400);
            let ledger = state.acoustic_ledger.lock().unwrap();
            assert_eq!(ledger.text_of(&occurrence), Some("recovered"));
            assert!(!ledger.is_sealed(&occurrence));
            assert!(ledger.text_recovery_pending(&occurrence));
            assert_eq!(ledger.layer_trail_for(&occurrence).count(), 3);
        }
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::LaneGone.code()),
            0
        );
        assert!(state.refinement_pending.is_empty());
        assert_eq!(state.tail_patch_awaiting_completion, 0);
    }

    #[test]
    fn retention_exhaustion_is_explicit_and_keeps_capture_pcm() {
        let (mut state, events, mut receiver, mut requests) = fixture(1);
        state.audio = LiveAudioBuffer::new(RATE, 1.0);
        state.audio.push(&vec![0.25; 2_000]);
        let ledger = closed(1);
        reconcile_silero_ledger(&mut state, &events, &ledger, &[]);
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::PcmUnavailable.code()),
            1
        );
        reconcile_silero_ledger(&mut state, &events, &ledger, &[]);
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::PcmUnavailable.code()),
            0
        );
        assert!(requests.try_recv().is_err());
        assert_eq!(state.audio.session_sample_end(), 2_000);
        assert_eq!(
            state.window_by_samples(1_000, 2_000).unwrap().samples,
            vec![0.25; 1_000]
        );
    }

    #[test]
    fn backlog_exhaustion_conserves_submitted_pending_and_failed_members() {
        let (mut state, events, mut receiver, _requests) = fixture(1);
        reconcile_silero_ledger(&mut state, &events, &closed(12), &[]);
        state.flush_layer1_coalesce(&events);
        assert_eq!(state.refinement_submitted.len(), 1);
        assert_eq!(state.refinement_pending.len(), LIVE_REFINEMENT_PENDING_CAP);
        assert_eq!(
            warnings(&mut receiver, RefinementFailure::BacklogExhausted.code()),
            3
        );
        assert_eq!(state.tail_patch_backpressure_drops, 3);
        assert_eq!(
            state.refinement_submitted.len()
                + state.refinement_pending.len()
                + state.tail_patch_backpressure_drops as usize,
            12
        );
    }

    #[test]
    fn stale_completion_cannot_return_the_current_observer() {
        let (mut state, events, _receiver, mut requests) = fixture(1);
        reconcile_silero_ledger(&mut state, &events, &closed(1), &[]);
        state.flush_layer1_coalesce(&events);
        let request = requests.try_recv().unwrap();
        let mut stale = finish(&request);
        stale.request_identity.as_mut().unwrap().range.capture_epoch += 1;
        state.complete_whisper_window(&events, stale, 20.0);
        assert_eq!(state.tail_patch_awaiting_completion, 1);
        assert_eq!(state.refinement_submitted.len(), 1);
        state.complete_whisper_window(&events, finish(&request), 20.0);
        state.complete_whisper_window(&events, finish(&request), 20.0);
        assert_eq!(state.tail_patch_awaiting_completion, 0);
        assert_eq!(state.tail_patch_jobs_skipped, 1);
    }

    #[test]
    fn stop_pressure_and_completion_race_classify_each_member_once() {
        for completion_first in [false, true] {
            let (mut state, events, mut receiver, mut requests) = fixture(1);
            reconcile_silero_ledger(&mut state, &events, &closed(3), &[]);
            state.flush_layer1_coalesce(&events);
            let request = requests.try_recv().unwrap();
            if completion_first {
                state.complete_whisper_window(&events, finish(&request), 20.0);
            }
            let deadline = state.refinement_clock + Duration::from_secs(1);
            assert!(!state.stop_refinements_tick(&events, deadline, deadline));
            let failures = warnings(&mut receiver, RefinementFailure::StopDeadline.code());
            assert_eq!(failures, if completion_first { 2 } else { 3 });
            state.complete_whisper_window(&events, finish(&request), 20.0);
            assert!(!state.stop_refinements_tick(&events, deadline, deadline));
            assert_eq!(
                warnings(&mut receiver, RefinementFailure::StopDeadline.code()),
                0
            );
            assert!(state.refinement_pending.is_empty());
            assert!(state.refinement_submitted.is_empty());
            assert_eq!(state.tail_patch_awaiting_completion, 0);
            state.seal_remaining_at_session_end(&events);
            let mut ledger = state.acoustic_ledger.lock().unwrap();
            let refusal = ledger
                .seal_terminal("live-admission", 7)
                .expect_err("no label at Stop");
            assert_eq!(refusal, SealRefusal::TextRecoveryPending);
            report_terminal_seal_refusal(&events, refusal);
            assert_eq!(
                warnings(&mut receiver, LEDGER_TERMINAL_SEAL_REFUSED_WARNING_CODE),
                1
            );
            assert_eq!(ledger.qualified_occurrences().count(), 3);
            assert_eq!(ledger.layer_trail().len(), 3);
            for occurrence in ledger.qualified_occurrences() {
                assert!(ledger.frontier_of(occurrence).unwrap().is_closed());
                assert!(!ledger.is_sealed(occurrence));
                assert!(ledger.text_of(occurrence).is_none());
            }
            let speech = crate::audio::capture_receipt::AcousticSpeechEvidence::measured(
                crate::audio::capture_receipt::CaptureEvidenceIdentity::new("live-admission", 7),
                "test_observer",
                crate::audio::capture_receipt::AcousticAvailability::Observed {
                    observed_samples: closed(3)
                        .utterances()
                        .iter()
                        .map(|u| u.range.sample_end)
                        .max()
                        .unwrap_or_default(),
                },
                closed(3)
                    .utterances()
                    .iter()
                    .map(|u| u.range.clone())
                    .collect::<Vec<_>>(),
            );
            let coverage = ledger.assess_seal_coverage("live-admission", 7, &speech, 250);
            assert_eq!(coverage.covered_samples, 0);
            assert_eq!(coverage.status, SealCoverageStatus::Incomplete);
            assert!(ledger.record_seal_coverage(coverage));
            assert_eq!(
                ledger.seal_terminal("live-admission", 7),
                Err(SealRefusal::TextRecoveryPending)
            );
            assert_eq!(state.sealed_count, 0);
            assert_eq!(
                state.pending_events.len(),
                3,
                "retain unresolved evidence for recovery"
            );
            assert_eq!(
                state.window_by_samples(0, 2_400).unwrap().samples,
                vec![0.25; 2_400]
            );
        }
    }
}

/// Relay L1 overlap admission. Phrase grain is one segment range, the shape
/// real decodes return (`4.0–12.0`). A word pin is a segment the recognizer
/// actually returned; this module does not split phrase text into words.
///
/// Phrase grain is one segment. A pin wholly inside the exclusive tail may
/// relabel that member. A pin wholly inside an already admitted range is
/// `replayed_range_identity`. A pin that still straddles stays whole,
/// visible, and without a second token.
#[cfg(test)]
mod relay_l1_overlap_admission_tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::{
        AcousticEvidence, EnergyCalibration, MutationReceipt, ObservationIdentity,
        ObservationProducer, OccurrenceIdentity,
    };
    use crate::stt::tail_provider::{
        TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
        TailProviderPayload, TailTimingQuality,
    };
    use tokio::sync::mpsc;

    const RATE: u32 = 16_000;

    struct Lane {
        state: AppleSealState,
        tx: mpsc::UnboundedSender<EngineEvent>,
        rx: mpsc::UnboundedReceiver<EngineEvent>,
        tail_rx: mpsc::Receiver<TailPatchRequest>,
    }

    fn open(session: &str) -> Lane {
        let (tx, rx) = mpsc::unbounded_channel();
        let (tail_tx, tail_rx) = mpsc::channel(8);
        let mut state = AppleSealState::new_for_session(RATE, session.to_string(), 1);
        state.tail_patch = Some(tail_tx);
        Lane {
            state,
            tx,
            rx,
            tail_rx,
        }
    }

    fn stage(lane: &mut Lane, utterance_id: u64, occurrence: OccurrenceIdentity, label: &str) {
        let calibration = EnergyCalibration {
            version: "relay-l1-overlap".to_string(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: occurrence.sample_len() as f64 * 1_000.0 / f64::from(RATE),
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(occurrence.sample_start),
            vad_close_sample: Some(occurrence.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        lane.state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .qualify(&evidence, &calibration);
        lane.state.energy_calibration = Some(calibration);
        assert!(
            admit_ledger_label(
                &mut lane.state,
                &lane.tx,
                LabelAdmission {
                    observation: ObservationIdentity::new(
                        ObservationProducer::Apple,
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
        lane.state.pending_events.insert(
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

    fn piece(utterance_id: u64, occurrence: &OccurrenceIdentity, text: &str) -> CoalescedPiece {
        let start_ts = occurrence.sample_start as f32 / RATE as f32;
        let end_ts = occurrence.sample_end as f32 / RATE as f32;
        CoalescedPiece {
            utterance_id,
            occurrence: occurrence.clone(),
            committed_text: text.to_string(),
            audio: vec![0.2; occurrence.sample_len() as usize],
            sample_start: occurrence.sample_start,
            sample_end: occurrence.sample_end,
            start_ts,
            covered_through_secs: end_ts,
            segment_count: 1,
        }
    }

    fn close_lexicon(
        lane: &mut Lane,
        utterance_id: u64,
        occurrence: &OccurrenceIdentity,
        label: &str,
    ) {
        assert!(
            admit_ledger_label(
                &mut lane.state,
                &lane.tx,
                LabelAdmission {
                    observation: ObservationIdentity::new(
                        ObservationProducer::Lexicon,
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

    fn segment(session: &str, text: &str, start: u64, end: u64) -> TimedTailSegment {
        TimedTailSegment {
            grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
            text: text.to_string(),
            range: crate::stt::tail_provider::TailSampleRange {
                session: session.to_string(),
                capture_epoch: 1,
                sample_start: start,
                sample_end: end,
            },
        }
    }

    fn word_pin(session: &str, text: &str, start: u64, end: u64) -> TimedTailSegment {
        let mut pin = segment(session, text, start, end);
        pin.grain = crate::stt::tail_provider::TailSegmentGrain::Word;
        pin
    }

    fn completion(
        request: &TailPatchRequest,
        segments: Vec<TimedTailSegment>,
    ) -> TailPatchCompletion {
        let text = segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let segment_grain = if !segments.is_empty()
            && segments
                .iter()
                .all(|segment| segment.grain == crate::stt::tail_provider::TailSegmentGrain::Word)
        {
            crate::stt::tail_provider::TailSegmentGrain::Word
        } else {
            crate::stt::tail_provider::TailSegmentGrain::Phrase
        };
        TailPatchCompletion {
            utterance_id: request.utterance_id,
            request_identity: Some(request.provider_request.identity.clone()),
            payload: Some(TailProviderPayload {
                identity: request.provider_request.identity.clone(),
                text,
                segments,
                avg_logprob: Some(-0.2),
                compression_ratio: Some(1.1),
                provider_id: TailProviderId::Fake,
                elapsed_ms: 1,
                evidence: TailProviderEvidence {
                    segment_grain,
                    source: TailEvidenceSource::Whisper,
                    revision: Some("relay-l1-phrase-grain".into()),
                    stability: TailEvidenceStability::Final,
                    timing_quality: TailTimingQuality::ExactSampleRange,
                    avg_logprob: Some(-0.2),
                },
            }),
            member_occurrences: request.member_occurrences.clone(),
        }
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn take_requests(rx: &mut mpsc::Receiver<TailPatchRequest>) -> Vec<TailPatchRequest> {
        let mut requests = Vec::new();
        while let Ok(request) = rx.try_recv() {
            requests.push(request);
        }
        requests
    }

    fn whisper_mutations(events: &[EngineEvent]) -> Vec<(String, MutationReceipt)> {
        events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::LedgerMutation {
                    observation,
                    label,
                    receipt,
                } if observation.producer == ObservationProducer::Whisper => {
                    Some((label.clone(), receipt.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn unanchored_label(events: &[EngineEvent], text: &str) -> bool {
        whisper_mutations(events)
            .into_iter()
            .any(|(label, receipt)| {
                label == text
                    && matches!(receipt, MutationReceipt::KeepVisibleUnanchored { .. })
                    && !receipt.grants_mutation()
            })
    }

    fn replay_refusal(events: &[EngineEvent], text: &str) -> bool {
        whisper_mutations(events)
            .into_iter()
            .any(|(label, receipt)| {
                label == text
                    && matches!(
                        receipt,
                        MutationReceipt::Refuse { reason, .. }
                            if reason.as_str() == "replayed_range_identity"
                    )
            })
    }

    fn named_refusal(events: &[EngineEvent], reason: &str) -> bool {
        whisper_mutations(events).into_iter().any(|(_, receipt)| {
            matches!(
                receipt,
                MutationReceipt::Refuse { reason: got, .. } if got.as_str() == reason
            )
        })
    }

    fn mutation_count(events: &[EngineEvent]) -> usize {
        whisper_mutations(events)
            .into_iter()
            .filter(|(_, receipt)| receipt.grants_mutation())
            .count()
    }

    fn assert_conserved(lane: &Lane, reason: Option<&str>) {
        let tally = lane
            .state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .conservation();
        assert_eq!(
            tally.observations_in, tally.receipts_out,
            "conservation counts offered observations and issued receipts apart"
        );
        assert!(tally.receipts_out > 0);
        if let Some(reason) = reason {
            assert!(
                tally.refusals_by_reason.get(reason).copied().unwrap_or(0) >= 1,
                "named refusal {reason} missing from {:?}",
                tally.refusals_by_reason
            );
        }
    }

    fn held_count(lane: &Lane) -> usize {
        lane.state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .occurrences()
            .count()
    }

    fn held_text(lane: &Lane, occurrence: &OccurrenceIdentity) -> Option<String> {
        lane.state
            .acoustic_ledger
            .lock()
            .expect("ledger")
            .text_of(occurrence)
            .map(str::to_owned)
    }

    fn launch_long(lane: &mut Lane, text: &str) -> (OccurrenceIdentity, Vec<TailPatchRequest>) {
        let occurrence = OccurrenceIdentity::new(lane.state.session_id.clone(), 1, 0, 160_000);
        stage(lane, 1, occurrence.clone(), text);
        assert!(
            lane.state
                .enqueue_layer1_piece(&lane.tx, piece(1, &occurrence, text))
        );
        close_lexicon(lane, 1, &occurrence, text);
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(requests.len(), 3, "step 1: 10 s becomes three 4 s windows");
        let windows = [(0, 64_000), (48_000, 112_000), (96_000, 160_000)];
        let admit = [(0, 48_000), (48_000, 96_000), (96_000, 160_000)];
        for (index, request) in requests.iter().enumerate() {
            let range = &request.provider_request.identity.range;
            assert_eq!(range.sample_start, windows[index].0);
            assert_eq!(range.sample_end, windows[index].1);
            assert_eq!(request.admit_sample_start, admit[index].0);
            assert_eq!(request.admit_sample_end, admit[index].1);
            assert_eq!(
                request.audio.len() as u64,
                range.sample_end - range.sample_start
            );
        }
        (occurrence, requests)
    }

    fn launch_coalesced(lane: &mut Lane) -> (Vec<OccurrenceIdentity>, Vec<TailPatchRequest>) {
        let session = lane.state.session_id.clone();
        let spans = [
            (0, 24_000, "alfa"),
            (24_000, 48_000, "beta"),
            (48_000, 72_000, "gamma"),
        ];
        let mut occurrences = Vec::new();
        for (index, (start, end, text)) in spans.into_iter().enumerate() {
            let occurrence = OccurrenceIdentity::new(session.clone(), 1, start, end);
            let id = (index as u64) + 1;
            stage(lane, id, occurrence.clone(), text);
            assert!(
                lane.state
                    .enqueue_layer1_piece(&lane.tx, piece(id, &occurrence, text))
            );
            close_lexicon(lane, id, &occurrence, text);
            occurrences.push(occurrence);
        }
        assert!(lane.state.flush_layer1_coalesce(&lane.tx));
        let _ = drain(&mut lane.rx);
        let requests = take_requests(&mut lane.tail_rx);
        assert_eq!(
            requests.len(),
            2,
            "two contiguous runs, second carries the prefix"
        );
        assert_eq!(requests[0].provider_request.identity.range.sample_start, 0);
        assert_eq!(
            requests[0].provider_request.identity.range.sample_end,
            48_000
        );
        assert_eq!(requests[0].admit_sample_start, 0);
        assert_eq!(requests[0].admit_sample_end, 48_000);
        assert_eq!(requests[0].member_occurrences.len(), 2);
        assert_eq!(
            requests[1].provider_request.identity.range.sample_start,
            32_000
        );
        assert_eq!(
            requests[1].provider_request.identity.range.sample_end,
            72_000
        );
        assert_eq!(requests[1].admit_sample_start, 48_000);
        assert_eq!(requests[1].admit_sample_end, 72_000);
        assert_eq!(requests[1].member_occurrences.len(), 1);
        assert_eq!(
            requests[1].audio.len() as u64,
            requests[1].provider_request.identity.range.sample_end
                - requests[1].provider_request.identity.range.sample_start
        );
        (occurrences, requests)
    }

    /// (a) One phrase pin across a long-occurrence slice boundary.
    ///
    /// Contract: step 3 (unanchored, never dropped), founding invariant
    /// (no word pins → read-only evidence, no duplicate token), forbidden
    /// `drop_acoustic_observation_without_receipt`, required receipt
    /// "observations unanchored (kept, no mutation right)".
    #[test]
    fn phrase_grain_segment_straddling_a_long_occurrence_slice_stays_visible_unanchored() {
        let mut lane = open("relay-long-phrase");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        let phrase = "od czwartej do siodmej";
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![segment("relay-long-phrase", phrase, 64_000, 112_000)],
            ),
            7.0,
        );
        let events = drain(&mut lane.rx);
        assert!(
            unanchored_label(&events, phrase),
            "step 3: a phrase pin across the 6 s slice boundary must stay visible as unanchored evidence, not vanish"
        );
        assert_eq!(
            held_count(&lane),
            1,
            "unanchored overlap must not mint a second token"
        );
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("cale zdanie")
        );
    }

    /// (b) One phrase pin across the coalesced prefix/admit boundary.
    ///
    /// Same contract lines as (a), plus Whisper truth: the overlap carries
    /// context and must not duplicate canvas content.
    #[test]
    fn phrase_grain_segment_straddling_a_coalesced_prefix_stays_visible_unanchored() {
        let mut lane = open("relay-coalesced-phrase");
        let (occurrences, requests) = launch_coalesced(&mut lane);
        let phrase = "przez granice";
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![segment("relay-coalesced-phrase", phrase, 40_000, 56_000)],
            ),
            4.5,
        );
        let events = drain(&mut lane.rx);
        assert!(
            unanchored_label(&events, phrase),
            "a phrase across the prefix/admit boundary stays visible without mutation authority"
        );
        assert_eq!(
            held_count(&lane),
            3,
            "the phrase must not become a fourth token"
        );
        assert_eq!(held_text(&lane, &occurrences[1]).as_deref(), Some("beta"));
        assert_eq!(held_text(&lane, &occurrences[2]).as_deref(), Some("gamma"));
    }

    /// (c) Later-window range already covered by an earlier admitted identity.
    ///
    /// Contract: step 4 `replayed_range_identity`. The refusal must not depend
    /// on the two strings matching. Required receipt: structural replays rejected.
    #[test]
    fn overlap_replay_covered_by_an_earlier_identity_is_refused() {
        let mut lane = open("relay-replay");
        let (occurrences, requests) = launch_coalesced(&mut lane);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![segment("relay-replay", "beta raz", 24_000, 48_000)],
            ),
            3.0,
        );
        let _ = drain(&mut lane.rx);
        assert_eq!(
            held_text(&lane, &occurrences[1]).as_deref(),
            Some("beta raz")
        );

        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![segment("relay-replay", "powtorka", 32_000, 48_000)],
            ),
            4.5,
        );
        let events = drain(&mut lane.rx);
        assert!(
            replay_refusal(&events, "powtorka"),
            "step 4: a covered range is replayed_range_identity even when the text differs"
        );
        assert_eq!(
            held_count(&lane),
            3,
            "replay must not mint a duplicate token"
        );
        assert_eq!(
            held_text(&lane, &occurrences[1]).as_deref(),
            Some("beta raz")
        );
        assert_eq!(held_text(&lane, &occurrences[2]).as_deref(), Some("gamma"));
    }

    /// One window of word pins does not relabel a longer occurrence.
    ///
    /// Contract step 7: bounded replacement addresses the whole span or nothing.
    /// A single exclusive slice, beside pins that straddle the admit bounds,
    /// is not that span. The Apple label stands and the straddling text stays
    /// unanchored.
    #[test]
    fn word_pins_on_one_window_do_not_relabel_the_long_occurrence() {
        let mut lane = open("relay-long-words");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    segment("relay-long-words", "krawedz", 40_000, 52_000),
                    segment("relay-long-words", "srodek", 52_000, 90_000),
                    segment("relay-long-words", "dalej", 90_000, 110_000),
                ],
            ),
            7.0,
        );
        let events = drain(&mut lane.rx);
        assert!(
            unanchored_label(&events, "krawedz"),
            "a word pin that straddles the earlier admit stays whole and unanchored"
        );
        assert!(
            unanchored_label(&events, "dalej"),
            "a word pin that straddles the next slice stays whole and unanchored"
        );
        assert!(
            unanchored_label(&events, "srodek"),
            "the exclusive slice stays visible and does not become the span label"
        );
        assert_eq!(
            mutation_count(&events),
            0,
            "step 7: one window must not replace the whole occurrence"
        );
        assert!(
            named_refusal(&events, "intersecting_pin_not_exclusive"),
            "a straddling pin refuses the replacement by name"
        );
        assert_eq!(held_count(&lane), 1);
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("cale zdanie")
        );
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    /// Coalesced prefix. A wholly covered word is replay. A pin that straddles
    /// into the later member blocks replacement of that member. The earlier
    /// member, wholly inside its own exclusive admit, may still be relabelled.
    ///
    /// Contract: step 4, step 7.
    #[test]
    fn word_pins_do_not_relabel_a_coalesced_member_across_a_straddle() {
        let mut lane = open("relay-coalesced-words");
        let (occurrences, requests) = launch_coalesced(&mut lane);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[0],
                vec![segment("relay-coalesced-words", "beta raz", 24_000, 48_000)],
            ),
            3.0,
        );
        let _ = drain(&mut lane.rx);

        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    segment("relay-coalesced-words", "przez", 40_000, 48_000),
                    segment("relay-coalesced-words", "krawedz", 46_000, 52_000),
                    segment("relay-coalesced-words", "ogon", 48_000, 56_000),
                ],
            ),
            4.5,
        );
        let events = drain(&mut lane.rx);
        assert!(
            replay_refusal(&events, "przez"),
            "a word pin wholly inside the already admitted prefix is replayed_range_identity"
        );
        assert!(
            unanchored_label(&events, "krawedz"),
            "a word pin across the admit boundary is kept whole, without a duplicate token"
        );
        assert!(
            unanchored_label(&events, "ogon"),
            "the exclusive tail stays visible when the member is not wholly proven"
        );
        assert!(
            named_refusal(&events, "intersecting_pin_not_exclusive"),
            "a pin straddling into the member refuses that member's replacement"
        );
        assert_eq!(
            held_text(&lane, &occurrences[2]).as_deref(),
            Some("gamma"),
            "step 7: the later member keeps its label"
        );
        assert_eq!(
            held_text(&lane, &occurrences[1]).as_deref(),
            Some("beta raz")
        );
        assert_eq!(held_count(&lane), 3);
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    /// (i) Three windows, every intersecting pin an exclusive tail.
    ///
    /// Contract step 7: the held text is the exclusive slices joined in PCM
    /// order, admitted once.
    #[test]
    fn three_exclusive_windows_join_the_whole_span_once() {
        let mut lane = open("relay-three-exclusive");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        let session = "relay-three-exclusive";
        let windows = [
            vec![segment(session, "raz", 8_000, 40_000)],
            vec![segment(session, "dwa", 52_000, 90_000)],
            vec![segment(session, "trzy", 100_000, 150_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        assert_eq!(
            mutation_count(&events),
            1,
            "the three slices are one admission"
        );
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("raz dwa trzy")
        );
        assert_eq!(held_count(&lane), 1);
        assert_conserved(&lane, None);
    }

    /// Long occurrence, overlapping windows, word pins.
    ///
    /// Contract step 4: a word whose range lies wholly outside this window's
    /// admit and inside an already admitted identity is `replayed_range_identity`.
    /// Contract step 5: the decision is the range, so the replayed word's text
    /// may differ from the word that owns that range in the later window.
    /// Contract step 7: words wholly inside each exclusive remainder join the
    /// occurrence once. The overlap must not mint a second token.
    #[test]
    fn exclusive_remainder_word_pins_join_once_and_prefix_words_are_replay() {
        let mut lane = open("relay-word-join");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        let session = "relay-word-join";
        let windows = [
            vec![
                word_pin(session, "raz", 8_000, 40_000),
                word_pin(session, "stary", 50_000, 62_000),
            ],
            vec![
                word_pin(session, "nowy", 50_000, 62_000),
                word_pin(session, "dwa", 70_000, 90_000),
                word_pin(session, "ogon", 100_000, 110_000),
            ],
            vec![word_pin(session, "trzy", 100_000, 150_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        assert!(
            replay_refusal(&events, "stary"),
            "step 4: the earlier window's word in the shared second is replay, whatever its text"
        );
        assert!(
            replay_refusal(&events, "ogon"),
            "step 4: a word past this window's admit is replay, not a second token"
        );
        assert_eq!(
            mutation_count(&events),
            1,
            "step 7: exclusive-remainder words join the whole span once"
        );
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("raz nowy dwa trzy")
        );
        assert_eq!(
            held_count(&lane),
            1,
            "replay must not mint a duplicate token"
        );
        assert_conserved(&lane, Some("replayed_range_identity"));
    }

    /// A word whose range crosses the prefix/remainder join stays one pin.
    ///
    /// Contract step 3 and step 7: the whole-span rule refuses it on the range
    /// alone. The pin's text matching the canvas is irrelevant.
    #[test]
    fn word_pin_straddling_the_prefix_join_stays_refused_on_range() {
        let mut lane = open("relay-word-straddle");
        let (occurrence, requests) = launch_long(&mut lane, "krawedz");
        let session = "relay-word-straddle";
        let windows = [
            vec![word_pin(session, "raz", 8_000, 40_000)],
            vec![
                word_pin(session, "krawedz", 40_000, 52_000),
                word_pin(session, "dwa", 52_000, 90_000),
            ],
            vec![word_pin(session, "trzy", 100_000, 150_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        assert!(
            unanchored_label(&events, "krawedz"),
            "a word across the admit join stays whole and visible"
        );
        assert!(
            named_refusal(&events, "intersecting_pin_not_exclusive"),
            "step 7: the straddle refuses the span on ranges, even when the text matches the canvas"
        );
        assert_eq!(mutation_count(&events), 0);
        assert_eq!(held_text(&lane, &occurrence).as_deref(), Some("krawedz"));
        assert_eq!(held_count(&lane), 1);
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    /// (ii) Three windows, one pin straddles an admit boundary.
    ///
    /// Contract step 7 and step 3: the held label is unchanged, the replacement
    /// is a named refusal, and the straddling text stays unanchored.
    #[test]
    fn one_straddling_pin_among_three_windows_refuses_the_span() {
        let mut lane = open("relay-three-straddle");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        let session = "relay-three-straddle";
        let windows = [
            vec![segment(session, "raz", 8_000, 40_000)],
            vec![
                segment(session, "krawedz", 40_000, 52_000),
                segment(session, "dwa", 52_000, 90_000),
            ],
            vec![segment(session, "trzy", 100_000, 150_000)],
        ];
        let mut events = Vec::new();
        for (request, segments) in requests.iter().zip(windows) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        assert_eq!(mutation_count(&events), 0);
        assert!(unanchored_label(&events, "krawedz"));
        assert!(named_refusal(&events, "intersecting_pin_not_exclusive"));
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("cale zdanie")
        );
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }

    /// (iii) Two of three windows return, then stop drains the accumulator.
    ///
    /// Contract step 10: no partial label. One named receipt. The returned
    /// texts stay visible.
    #[test]
    fn stop_with_two_of_three_windows_keeps_the_label() {
        let mut lane = open("relay-stop-partial");
        let (occurrence, requests) = launch_long(&mut lane, "cale zdanie");
        let session = "relay-stop-partial";
        let mut events = Vec::new();
        for (request, segments) in requests.iter().take(2).zip([
            vec![segment(session, "raz", 8_000, 40_000)],
            vec![segment(session, "dwa", 52_000, 90_000)],
        ]) {
            lane.state
                .complete_whisper_window(&lane.tx, completion(request, segments), 8.0);
            events.extend(drain(&mut lane.rx));
        }
        lane.state
            .return_outstanding_whisper_without_label(&lane.tx);
        events.extend(drain(&mut lane.rx));
        assert_eq!(mutation_count(&events), 0);
        assert!(unanchored_label(&events, "raz"));
        assert!(unanchored_label(&events, "dwa"));
        assert!(named_refusal(&events, "incomplete_exclusive_coverage"));
        assert_eq!(
            held_text(&lane, &occurrence).as_deref(),
            Some("cale zdanie")
        );
        assert_conserved(&lane, Some("incomplete_exclusive_coverage"));
    }

    /// (iv) A coalesced member keeps its label when a pin straddles into it.
    ///
    /// Contract step 7: the exclusive word in that member is not a licence to
    /// replace the member.
    #[test]
    fn coalesced_member_with_a_straddling_pin_keeps_its_label() {
        let mut lane = open("relay-member-straddle");
        let (occurrences, requests) = launch_coalesced(&mut lane);
        lane.state.complete_whisper_window(
            &lane.tx,
            completion(
                &requests[1],
                vec![
                    segment("relay-member-straddle", "krawedz", 46_000, 52_000),
                    segment("relay-member-straddle", "ogon", 48_000, 56_000),
                ],
            ),
            4.5,
        );
        let events = drain(&mut lane.rx);
        assert!(unanchored_label(&events, "krawedz"));
        assert!(unanchored_label(&events, "ogon"));
        assert!(named_refusal(&events, "intersecting_pin_not_exclusive"));
        assert_eq!(mutation_count(&events), 0);
        assert_eq!(held_text(&lane, &occurrences[2]).as_deref(), Some("gamma"));
        assert_eq!(held_count(&lane), 3);
        assert_conserved(&lane, Some("intersecting_pin_not_exclusive"));
    }
}
