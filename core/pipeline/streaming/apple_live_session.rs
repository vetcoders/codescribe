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

use crate::pipeline::contracts::{DropKind, EngineEvent, EventSink, TranscriptSegment};
use crate::pipeline::stream_postprocess::StreamPostProcessor;
use crate::stt::apple_stt::{LiveStreamEvent, LiveStreamSession};
use crate::stt::tail_patcher::{TailPatchConfig, TailPatchOutcome};

use super::live_audio_buffer::{DEFAULT_RETENTION_SECS, LiveAudioBuffer};
use super::session::{
    SessionConfig, compute_tail_patch_job, emit_session_finalised, emit_tail_patch_result,
    tail_patch_enabled,
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

/// One sealed utterance handed from the worker thread to the async Layer 1 lane.
struct TailPatchRequest {
    utterance_id: u64,
    /// Byte-identical to the emitted `UtteranceFinal.text` — the string every
    /// `ReplaceRange` char offset is computed against.
    committed_text: String,
    /// PCM behind exactly this utterance: `[previous seal end, end_ts)`.
    audio: Vec<f32>,
}

/// Async Layer 1 lane for the Apple progressive path.
///
/// Owns the in-flight Whisper gap-fill job and the replacement count that
/// `SessionFinalised.layer_summary` reports. Jobs are boxed so the lane can be
/// driven by a stub future in tests without a model on disk.
struct AppleTailPatchLane {
    jobs: FuturesOrdered<BoxFuture<'static, Result<(u64, TailPatchOutcome)>>>,
    sample_rate: u32,
    language: Option<String>,
    config: TailPatchConfig,
    replacements: u64,
}

impl AppleTailPatchLane {
    /// Open an empty lane. `TailPatchConfig::from_env` is read once here so the
    /// whole session judges every patch against the same thresholds, even if the
    /// env flips mid-hold.
    fn new(sample_rate: u32, language: Option<String>) -> Self {
        Self {
            jobs: FuturesOrdered::new(),
            sample_rate,
            language,
            // F2: thresholds stay exactly where the shared primitive puts them.
            config: TailPatchConfig::from_env(),
            replacements: 0,
        }
    }

    /// Turn a sealed utterance into a Whisper gap-fill job and queue it. The job
    /// is only constructed — inference happens inside it on `spawn_blocking`, so
    /// this call never sits on the event-drain path.
    fn push_request(&mut self, req: TailPatchRequest) {
        let job = compute_tail_patch_job(
            req.utterance_id,
            req.committed_text,
            req.audio,
            self.sample_rate,
            self.language.clone(),
            self.config,
        );
        self.push_job(Box::pin(job));
    }

    /// Queue an already-built job. Boxed and separate from `push_request` so
    /// tests can drive the lane with a stub future, with no model on disk.
    fn push_job(&mut self, job: BoxFuture<'static, Result<(u64, TailPatchOutcome)>>) {
        self.jobs.push_back(job);
    }

    /// Await the next finished job. `FuturesOrdered` (not `Unordered`) is the
    /// point: patches must reach the sink in seal order, or a later utterance's
    /// `ReplaceRange` could land before an earlier one's.
    async fn next(&mut self) -> Option<Result<(u64, TailPatchOutcome)>> {
        self.jobs.next().await
    }

    /// Emit a finished job's patch and fold its replacement count into the
    /// session total. A skipped or failed patch contributes zero — only text
    /// that actually reached the canvas is counted.
    fn complete(&mut self, event_sink: &dyn EventSink, result: Result<(u64, TailPatchOutcome)>) {
        self.replacements = self
            .replacements
            .saturating_add(emit_tail_patch_result(event_sink, result));
    }

    /// How many bounded replacements Layer 1 landed this session — the number
    /// `SessionFinalised.layer_summary` reports.
    fn replacements(&self) -> u64 {
        self.replacements
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
    } = config;
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
    // Bounded: the worker `try_send`s from the PCM-forwarding thread.
    let (tp_tx, mut tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
    // Layered off → the worker gets no sender at all, so the lane stays empty
    // and its branch never yields: zero jobs, zero behaviour change.
    let worker_tp_tx = tail_patch_on.then_some(tp_tx);

    let worker = thread::spawn(move || {
        apple_stream_worker(
            pcm_rx,
            ev_tx,
            sample_rate,
            language.as_deref(),
            worker_tp_tx,
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
            // Admit one sealed utterance into Layer 1 at a time. The Whisper
            // call itself runs on `spawn_blocking` inside the job, so this loop
            // only ever schedules and collects — inference never sits on the
            // event-drain path (F1).
            Some(req) = tp_rx.recv(), if !tail_patch_in_flight => {
                tail_patch_lane.push_request(req);
                tail_patch_in_flight = true;
            }
            Some(result) = tail_patch_lane.next() => {
                tail_patch_in_flight = false;
                // Flush everything the worker already queued first: a patch
                // must never overtake the `UtteranceFinal` it patches.
                while let Ok(event) = ev_rx.try_recv() {
                    deliver_event(&event, event_sink.as_ref(), stream_log_path.as_deref());
                }
                tail_patch_lane.complete(event_sink.as_ref(), result);
            }
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
            tail_patch_lane.complete(event_sink.as_ref(), result);
        }
        match tp_rx.try_recv() {
            Ok(req) => {
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

    match worker.join() {
        Ok(Ok(outcome)) => {
            info!(
                sealed = outcome.sealed,
                filtered_empty_drops = outcome.filtered_empty_drops,
                unresolved_windows = outcome.unresolved_windows,
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
struct AppleSealState {
    postprocessor: StreamPostProcessor,
    preview_rev: u64,
    utterance_id: u64,
    open_partial: String,
    sealed_count: u64,
    filtered_empty_drops: u64,
    /// Bounded PCM retention, so a sealed boundary can be resolved back to the
    /// audio behind it (Layer 1 tail-patch prerequisite).
    audio: LiveAudioBuffer,
    /// Session time of the previous seal — the lower bound of the next
    /// utterance's audio window.
    last_sealed_end: f32,
    /// Seals whose audio window could not be resolved (F3 falsification).
    unresolved_windows: u64,
    /// Layer 1 hand-off, present only when layered transcription is armed.
    tail_patch: Option<mpsc::Sender<TailPatchRequest>>,
    /// Seals whose tail-patch request found the queue full (F1 backpressure).
    tail_patch_backpressure_drops: u64,
    /// Concatenation of already progressive-sealed text — left context for
    /// Light+ casing on the next seal (w2-b).
    sealed_prefix: String,
}

impl AppleSealState {
    /// Fresh seal state with Layer 1 disabled (`tail_patch: None`) — the default
    /// shape when `CODESCRIBE_LAYERED_TRANSCRIPTION` is unset.
    fn new(sample_rate: u32) -> Self {
        Self {
            postprocessor: StreamPostProcessor::new(),
            preview_rev: 0,
            utterance_id: 0,
            open_partial: String::new(),
            sealed_count: 0,
            filtered_empty_drops: 0,
            audio: LiveAudioBuffer::new(sample_rate, DEFAULT_RETENTION_SECS),
            last_sealed_end: 0.0,
            unresolved_windows: 0,
            tail_patch: None,
            tail_patch_backpressure_drops: 0,
            sealed_prefix: String::new(),
        }
    }

    /// Same state, armed with the Layer 1 hand-off. Holding the sender is what
    /// makes `seal_utterance_final` clone the committed text at all — with no
    /// wire there is nothing to diff against later.
    fn new_with_tail_patch(sample_rate: u32, tail_patch: mpsc::Sender<TailPatchRequest>) -> Self {
        Self {
            tail_patch: Some(tail_patch),
            ..Self::new(sample_rate)
        }
    }
}

/// What the worker sealed, and what seal-time postprocess filtered away.
struct AppleStreamOutcome {
    sealed: u64,
    filtered_empty_drops: u64,
    unresolved_windows: u64,
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
fn resolve_sealed_audio_window(state: &mut AppleSealState, end_ts: f32) -> Option<Vec<f32>> {
    let from = state.last_sealed_end;
    match state.audio.window(from, end_ts) {
        Some(window) => {
            state.last_sealed_end = end_ts;
            // Everything before this utterance is committed canvas; no future
            // patch reaches back past it.
            state.audio.committed_through(from);
            tracing::debug!(
                from_secs = from,
                end_ts,
                window_samples = window.len(),
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
    start_ts: f32,
    end_ts: f32,
    segments: Vec<TranscriptSegment>,
) -> bool {
    let raw_text = raw.trim().to_string();
    if raw_text.is_empty() {
        return false;
    }

    let Some(corrected) = state.postprocessor.process_utterance(&raw_text) else {
        state.filtered_empty_drops = state.filtered_empty_drops.saturating_add(1);
        warn!(
            raw_text = %raw_text,
            "Apple seal dropped: empty after lexicon/cleanup"
        );
        let _ = ev_tx.send(EngineEvent::Drop {
            kind: DropKind::FilteredEmpty,
            text: raw_text,
            reason: "Empty after lexicon/cleanup (not semantic gate)".to_string(),
        });
        return false;
    };

    // TRIM CONTRACT parity with the VAD path: the emitted final is the string
    // future ReplaceRange char offsets are computed against, so it must be
    // trimmed here and nowhere else.
    //
    // Progressive seal (w2-b): lexicon (above) then Light+ with left context so
    // casing at a sentence start sees the preceding sealed terminal. Order is
    // fixed — replacements before casing. Force-raw skips Light+ on the stop
    // path (Ctrl-hold); at seal time the live canvas always shapes so residual
    // stop no longer needs a full-file punctuation re-decode.
    let after_lexicon = corrected.trim().to_string();
    let text =
        crate::pipeline::light_plus::apply_with_left_context(&state.sealed_prefix, &after_lexicon);
    if !state.sealed_prefix.is_empty() && !text.is_empty() {
        state.sealed_prefix.push(' ');
    }
    state.sealed_prefix.push_str(&text);
    state.utterance_id = state.utterance_id.saturating_add(1);
    state.sealed_count = state.sealed_count.saturating_add(1);
    // Layer 1 diffs against this exact string, so capture it before the event
    // takes ownership — and only when there is a lane to hand it to.
    let committed_text = state.tail_patch.is_some().then(|| text.clone());
    let utterance_id = state.utterance_id;
    let _ = ev_tx.send(EngineEvent::UtteranceFinal {
        utterance_id,
        text,
        raw_text,
        start_ts,
        end_ts,
        segments,
        vad_speech_pct: None,
        avg_logprob: None,
        compression_ratio: None,
        quality_gate_dropped: false,
        confidence_flags: Vec::new(),
    });

    let window = resolve_sealed_audio_window(state, end_ts);
    // The seal path runs on the PCM-forwarding thread: hand off without ever
    // waiting. A full queue costs a patch, never a stalled capture (F1).
    if let (Some(audio), Some(committed_text)) = (window, committed_text) {
        let send = state.tail_patch.as_ref().map(|tx| {
            tx.try_send(TailPatchRequest {
                utterance_id,
                committed_text,
                audio,
            })
        });
        if let Some(Err(e)) = send {
            state.tail_patch_backpressure_drops =
                state.tail_patch_backpressure_drops.saturating_add(1);
            warn!(
                utterance_id,
                "Layer 1 tail-patch request dropped — queue full or lane gone: {e}"
            );
        }
    }
    true
}

/// Blocking worker: owns the stream session for its full lifetime.
fn apple_stream_worker(
    pcm_rx: std_mpsc::Receiver<Option<Vec<f32>>>,
    ev_tx: mpsc::UnboundedSender<EngineEvent>,
    sample_rate: u32,
    language: Option<&str>,
    tail_patch: Option<mpsc::Sender<TailPatchRequest>>,
) -> anyhow::Result<AppleStreamOutcome> {
    let mut stream = LiveStreamSession::open(language, sample_rate)?;
    let mut state = match tail_patch {
        Some(tx) => AppleSealState::new_with_tail_patch(sample_rate, tx),
        None => AppleSealState::new(sample_rate),
    };
    let mut samples_seen: u64 = 0;

    loop {
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
    let trailing = stream.finish()?;
    emit_stream_events(trailing, &ev_tx, &mut state, audio_secs);

    // Seal open partial that never got a phrase final (stop mid-phrase).
    // Same seal-time correction as the phrase path — a stop mid-utterance must
    // not be the one route that commits uncorrected text.
    let open = state.open_partial.trim().to_string();
    if !open.is_empty() {
        seal_utterance_final(&mut state, &ev_tx, &open, 0.0, audio_secs, Vec::new());
    }

    Ok(AppleStreamOutcome {
        sealed: state.sealed_count,
        filtered_empty_drops: state.filtered_empty_drops,
        unresolved_windows: state.unresolved_windows,
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
fn emit_stream_events(
    events: Vec<LiveStreamEvent>,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    state: &mut AppleSealState,
    audio_secs: f32,
) {
    for event in events {
        match event {
            LiveStreamEvent::Ready | LiveStreamEvent::End => {}
            LiveStreamEvent::Partial { text } => {
                // Previews stay RAW: they are in-flight presentation, not
                // canvas, and correcting them would make the lexicon rewrite
                // flicker letter by letter while the phrase is still forming.
                state.open_partial = text.clone();
                state.preview_rev = state.preview_rev.saturating_add(1);
                let _ = ev_tx.send(EngineEvent::Preview {
                    rev: state.preview_rev,
                    text,
                });
            }
            LiveStreamEvent::PhraseFinal { text, segments } => {
                // The phrase is closed either way — the open partial is stale.
                state.open_partial.clear();
                let start_ts = segments.first().map(|s| s.start_ts).unwrap_or(0.0);
                let end_ts = segments.last().map(|s| s.end_ts).unwrap_or(audio_secs);
                seal_utterance_final(state, ev_tx, &text, start_ts, end_ts, segments);
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
                if state.sealed_count == 0 {
                    if seal_utterance_final(state, ev_tx, &text, 0.0, audio_secs, segments) {
                        state.open_partial.clear();
                    }
                } else {
                    // Phrase seals already emitted; don't double-seal open partial.
                    state.open_partial.clear();
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

    /// Partial → Preview; each phrase final → UtteranceFinal with rising ids.
    #[test]
    fn emit_maps_partial_and_two_phrase_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new(TEST_SAMPLE_RATE);
        emit_stream_events(
            vec![
                LiveStreamEvent::Partial {
                    text: "hello".into(),
                },
                LiveStreamEvent::PhraseFinal {
                    text: "hello world".into(),
                    segments: vec![],
                },
                LiveStreamEvent::PhraseFinal {
                    text: "second".into(),
                    segments: vec![],
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
                segments: vec![],
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
                segments: vec![],
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
                segments: vec![],
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
                segments: vec![],
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
        lane.push_job(Box::pin(async move { Ok((1u64, outcome)) }));

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
        lane.push_job(Box::pin(async move { Ok((1u64, outcome)) }));

        let result = lane.next().await.expect("one job in flight");
        lane.complete(&sink, result);

        assert_eq!(lane.replacements(), 0);
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
}
