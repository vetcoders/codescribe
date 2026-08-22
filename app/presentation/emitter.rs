//! Event-driven presentation emitter.
//!
//! Converts `EngineEvent`s into user-facing output by delegating to
//! `BufferedEmitter` (typing animation, delta encoding) from core.
//!
//! Uses an ordered mpsc channel to guarantee that target updates and finish
//! arrive in the exact order they were emitted,
//! eliminating the fire-and-forget tokio::spawn ordering race.

use std::sync::Arc;

use codescribe_core::pipeline::contracts::{DeltaSink, EngineEvent, EventSink, TranscriptSegment};
use codescribe_core::pipeline::streaming::BufferedEmitter;
use tokio::sync::Mutex;
use tracing::{debug, info};

use super::transcript_bus::{TranscriptBus, TranscriptDraft, TranscriptDraftStatus};

/// Commands sent through the ordered channel to the emitter worker.
enum EmitterCmd {
    SetTargetText(String),
    Finish,
}

/// What the delta sink is shown on every update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeltaRenderMode {
    /// Whole session so far: every committed utterance plus the live preview
    /// tail. Hands-off dictation — the transcript accumulates, never replaces.
    #[default]
    SessionRendered,
    /// Only the live preview. Assistive hold-to-talk, where each utterance is
    /// its own delivery and carrying earlier text forward would re-insert it.
    ActivePreviewOnly,
}

/// One mutable engine-finalized utterance. `text` is the working string every later
/// `ReplaceRange` / `InsertAnnotation` char offset is computed against;
/// `raw_text` keeps the uncorrected engine output for the quality loop.
#[derive(Debug, Clone, PartialEq)]
struct TranscriptUtteranceRecord {
    utterance_id: u64,
    text: String,
    raw_text: String,
    start_ts: f32,
    end_ts: f32,
    segments: Vec<TranscriptSegment>,
}

impl TranscriptUtteranceRecord {
    /// Narrow the reducer's internal record to the clean public bus contract.
    /// `raw_text` is deliberately excluded at this boundary.
    fn clean_draft(&self) -> TranscriptDraft {
        TranscriptDraft {
            utterance_id: self.utterance_id,
            text: self.text.clone(),
            start_seconds: self.start_ts,
            end_seconds: self.end_ts,
            segments: self.segments.clone(),
        }
    }
}

/// Source of truth for the session transcript: everything already committed,
/// plus the in-flight preview tail.
///
/// The split matters. `committed` is canvas — append-only, patched in place but
/// never rewritten wholesale. `active_preview` is presentation that has not
/// earned canvas yet and is replaced freely. `last_non_empty_preview` is the
/// fallback for a final that arrives empty (VAD sealed on a quiet tail), so a
/// real utterance is not lost to a blank final.
#[derive(Debug, Default)]
pub struct TranscriptReducer {
    committed: Vec<TranscriptUtteranceRecord>,
    active_preview: String,
    last_non_empty_preview: String,
}

/// Trim a fragment's outer edges. Interior whitespace and newlines survive —
/// the renderer receives markdown, so collapsing them would flatten structure.
fn normalize_transcript_fragment(text: &str) -> String {
    text.trim().to_string()
}

/// Append a fragment to the rendered buffer, inserting a single separating
/// space only when one is actually needed. Empty fragments are skipped, so a
/// blank preview cannot leave trailing whitespace on the canvas.
fn append_rendered_fragment(rendered: &mut String, fragment: &str) {
    let normalized = normalize_transcript_fragment(fragment);
    if normalized.is_empty() {
        return;
    }

    if !rendered.is_empty() && !rendered.ends_with(char::is_whitespace) {
        rendered.push(' ');
    }
    rendered.push_str(&normalized);
}

impl TranscriptReducer {
    /// Replace the live preview tail. Previews supersede each other, so this
    /// overwrites rather than appends; a non-empty preview is also remembered as
    /// the fallback an empty final will fall back to.
    fn apply_preview(&mut self, text: &str) {
        let normalized = normalize_transcript_fragment(text);
        self.active_preview = normalized.clone();
        if !normalized.is_empty() {
            self.last_non_empty_preview = normalized;
        }
    }

    /// Route a correction to whatever it actually targets.
    ///
    /// A correction can arrive after its utterance was already finalized, so
    /// when no preview is open the committed list is searched from the tail for
    /// the exact `previous_text` and patched in place. Only an unmatched
    /// correction falls through to the preview path — treating it as new
    /// content. Without the search, a late correction to a non-tail utterance
    /// would append a duplicate instead of fixing the original.
    fn apply_correction(&mut self, previous_text: &str, text: &str) -> Option<usize> {
        let previous = normalize_transcript_fragment(previous_text);
        let corrected = normalize_transcript_fragment(text);

        // Over-correct for P3-03 (late correction to penultimate/older utterance):
        // search committed from the tail for a match and patch it. This prevents
        // append-dupe when a correction for non-tail arrives after its finalize.
        // Only falls back to preview-append if no match found (new content).
        if self.active_preview.is_empty() {
            // Fast path + P3-03: search from tail (last first). Collapsed if for clippy.
            for (index, rec) in self.committed.iter_mut().enumerate().rev() {
                if normalize_transcript_fragment(&rec.text) == previous {
                    rec.text = corrected;
                    return Some(index);
                }
            }
        }

        self.apply_preview(&corrected);
        None
    }

    /// Test helper: delete chars from the live preview tail only.
    #[cfg(test)]
    fn backspace_active_preview(&mut self, delete_count: usize) {
        for _ in 0..delete_count {
            self.active_preview.pop();
        }
        if !self.active_preview.is_empty() {
            self.last_non_empty_preview = self.active_preview.clone();
        }
    }

    /// Promote the current utterance to committed canvas and return the text
    /// handed to the utterance callback (`None` when there was nothing to
    /// commit).
    ///
    /// An empty `text` falls back to the last non-empty preview, so an utterance
    /// the engine sealed blank is still delivered. Both preview fields are
    /// cleared either way — the tail belongs to this utterance and must not leak
    /// into the next one.
    fn finalize(
        &mut self,
        utterance_id: u64,
        text: &str,
        raw_text: &str,
        start_ts: f32,
        end_ts: f32,
        segments: Vec<TranscriptSegment>,
    ) -> Option<String> {
        let committed_text = {
            let normalized = normalize_transcript_fragment(text);
            if normalized.is_empty() {
                self.last_non_empty_preview.clone()
            } else {
                normalized
            }
        };

        self.active_preview.clear();
        self.last_non_empty_preview.clear();

        if committed_text.is_empty() {
            return None;
        }

        if let Some(existing) = self
            .committed
            .iter_mut()
            .find(|record| record.utterance_id == utterance_id)
        {
            existing.text = committed_text;
            existing.raw_text = raw_text.to_string();
            existing.start_ts = start_ts;
            existing.end_ts = end_ts;
            existing.segments = segments;
            return None;
        }

        self.committed.push(TranscriptUtteranceRecord {
            utterance_id,
            text: committed_text.clone(),
            raw_text: raw_text.to_string(),
            start_ts,
            end_ts,
            segments,
        });
        Some(committed_text)
    }

    /// Drop the in-flight preview without committing it — used when the engine
    /// reports no speech, and at session end so an uncommitted tail does not
    /// outlive the finalized utterances.
    fn clear_live_preview(&mut self) {
        self.active_preview.clear();
        self.last_non_empty_preview.clear();
    }

    /// Render the whole session: committed utterances in order, then the live
    /// preview tail. Rebuilt from state on every call, so the rendered string is
    /// always a function of the record list rather than an accumulated buffer
    /// that could drift from it.
    pub fn rendered_text(&self) -> String {
        let mut rendered = String::new();
        for utterance in &self.committed {
            append_rendered_fragment(&mut rendered, &utterance.text);
        }
        append_rendered_fragment(&mut rendered, &self.active_preview);
        rendered
    }

    /// Apply an ADR bounded patch (`ReplaceRange` / `InsertAnnotation`) to the
    /// committed utterance it targets, so the authoritative transcript
    /// (`transcript_buffer` → paste/history) reflects the same correction the
    /// overlay receives. Offsets are char offsets within `utterance_id` (see
    /// `EngineEvent::apply_to_committed_text`). Returns whether the buffer
    /// changed — `false` when the utterance is not (yet) committed or the offsets
    /// fall outside it (the patch is dropped rather than corrupting the buffer).
    fn apply_layered_patch(&mut self, event: &EngineEvent) -> bool {
        let utterance_id = match event {
            EngineEvent::ReplaceRange { utterance_id, .. }
            | EngineEvent::InsertAnnotation { utterance_id, .. } => *utterance_id,
            _ => return false,
        };
        let Some(record) = self
            .committed
            .iter_mut()
            .rfind(|record| record.utterance_id == utterance_id)
        else {
            return false;
        };
        // Last-mile duplicate guard. A patch is computed against the canvas as
        // it stood when Layer 1 was dispatched; by the time it arrives SFSpeech
        // may have restated the SAME utterance at greater length, already
        // delivering the words the patch recovers. Measured 2026-08-14: an
        // append computed for a 15-character canvas landed on the 47-character
        // restatement of it and duplicated the phrase ("…hard pruna I road
        // która pozwoli nam na zrobienie hard Pru."), costing more WER than the
        // recovery gained. Only pure insertions are checked — a substitution
        // replaces the very span it would be compared against.
        if let EngineEvent::ReplaceRange {
            start, end, text, ..
        } = event
            && start == end
            && codescribe_core::stt::tail_patcher::text_already_carries(&record.text, text)
        {
            tracing::debug!(
                utterance_id,
                "layered patch already carried by the canvas; dropped"
            );
            return false;
        }
        match event.apply_to_committed_text(&mut record.text) {
            Ok(applied) => applied,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    utterance_id,
                    "layered transcript patch offsets out of range; dropped"
                );
                false
            }
        }
    }

    /// Test helper: read committed utterance records without mut access.
    #[cfg(test)]
    fn committed(&self) -> &[TranscriptUtteranceRecord] {
        &self.committed
    }

    /// Apply one engine event using the exact transcript algebra owned by the
    /// shipped presentation emitter. The returned text is present only when a
    /// new final slot was inserted; same-id revisions update that slot without
    /// dispatching a second per-utterance callback.
    pub fn apply_event(&mut self, event: &EngineEvent) -> Option<String> {
        match event {
            EngineEvent::Preview { text, .. } => self.apply_preview(text),
            EngineEvent::Correction {
                text,
                previous_text,
                ..
            } => {
                let _ = self.apply_correction(previous_text, text);
            }
            EngineEvent::UtteranceFinal {
                utterance_id,
                text,
                raw_text,
                start_ts,
                end_ts,
                segments,
                ..
            } => {
                return self.finalize(
                    *utterance_id,
                    text,
                    raw_text,
                    *start_ts,
                    *end_ts,
                    segments.clone(),
                );
            }
            EngineEvent::ReplaceRange { .. } | EngineEvent::InsertAnnotation { .. } => {
                let _ = self.apply_layered_patch(event);
            }
            EngineEvent::SidebandEvidence { .. } => {
                // Timing evidence is not transcript authority.
            }
            EngineEvent::NoSpeech { .. } => self.clear_live_preview(),
            _ => {}
        }
        None
    }

    /// Finalized canvas only, excluding the volatile preview tail.
    pub fn streaming_floor(&self) -> String {
        let mut rendered = String::new();
        for utterance in &self.committed {
            append_rendered_fragment(&mut rendered, &utterance.text);
        }
        rendered
    }

    /// Number of unique finalized slots currently held by the reducer.
    pub fn committed_count(&self) -> usize {
        self.committed.len()
    }
}

/// Replay an ordered event vector through the production presentation algebra.
pub fn reduce_transcript_events(events: &[EngineEvent]) -> TranscriptReducer {
    let mut reducer = TranscriptReducer::default();
    for event in events {
        let _ = reducer.apply_event(event);
    }
    reducer
}

#[cfg(test)]
type SessionTranscriptState = TranscriptReducer;

/// Presentation emitter — bridges `EngineEvent`s to `BufferedEmitter`.
///
/// Implements `EventSink` so it can be plugged directly into `transcription_session`.
/// Internally manages the `BufferedEmitter` tick loop for typing animation.
///
/// All mutations to `BufferedEmitter` are serialized through an mpsc channel,
/// guaranteeing in-order delivery (no fire-and-forget spawn races).
pub struct PresentationEmitter {
    cmd_tx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<EmitterCmd>>>,
    emitter_handle: Option<tokio::task::JoinHandle<()>>,
    cmd_handle: Option<tokio::task::JoinHandle<()>>,
    /// Optional callback for completed utterances (used by Toggle mode).
    utterance_callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    /// Optional callback for VAD stop detection.
    vad_start_callback: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Optional callback for VAD end/silence boundary detection.
    vad_end_callback: Option<Arc<dyn Fn() + Send + Sync>>,
    vad_start_emitted: std::sync::atomic::AtomicBool,
    /// Source-of-truth transcript state: committed utterances + active preview tail.
    session_state: std::sync::Mutex<TranscriptReducer>,
    /// Controls what the delta sink sees: full session text or only the live preview.
    delta_render_mode: DeltaRenderMode,
    /// Durable observer of this exact reducer's committed/final truth.
    transcript_bus: Option<Arc<TranscriptBus>>,
}

impl PresentationEmitter {
    /// Build the emitter and start both background tasks: the `BufferedEmitter`
    /// tick loop (typing animation) and the FIFO command worker.
    ///
    /// Every mutation goes through the command channel, which is what removes
    /// the fire-and-forget spawn ordering race — target updates and finish
    /// arrive in emit order. The worker catches panics from the emitter so a
    /// poisoned animation forces a clean finish instead of leaving the tick loop
    /// running forever.
    pub fn new(
        transcript_buffer: Arc<Mutex<String>>,
        delta_callback: Option<Arc<dyn DeltaSink>>,
        stream_log_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self::new_with_transcript_bus(transcript_buffer, delta_callback, stream_log_path, None)
    }

    /// Build an emitter observed by the clean transcript bus. The bus sees the
    /// same reducer mutation as paste/history and never reconstructs UI deltas.
    pub fn new_with_transcript_bus(
        transcript_buffer: Arc<Mutex<String>>,
        delta_callback: Option<Arc<dyn DeltaSink>>,
        stream_log_path: Option<std::path::PathBuf>,
        transcript_bus: Option<Arc<TranscriptBus>>,
    ) -> Self {
        let emitter = Arc::new(Mutex::new(BufferedEmitter::new(
            transcript_buffer,
            delta_callback,
            stream_log_path,
        )));

        let emitter_clone = emitter.clone();
        let emitter_handle = Some(tokio::spawn(
            codescribe_core::pipeline::streaming::emitter_tick_loop(emitter_clone),
        ));

        // Ordered command channel: on_event sends commands, worker processes in FIFO order.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EmitterCmd>();
        let emitter_for_cmd = emitter.clone();
        let cmd_handle = Some(tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                let mut guard = emitter_for_cmd.lock().await;
                let should_break = matches!(&cmd, EmitterCmd::Finish);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match cmd {
                    EmitterCmd::SetTargetText(text) => guard.set_target_text(text),
                    EmitterCmd::Finish => {
                        guard.finish();
                        None
                    }
                }));
                let mut panicked = false;
                match result {
                    Ok(Some(snapshot)) => {
                        guard.store_transcript_snapshot(snapshot).await;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        panicked = true;
                    }
                }
                if panicked {
                    tracing::error!("Emitter command worker panicked; forcing emitter finish");
                    guard.finish();
                    break;
                }
                if should_break {
                    break;
                }
            }
            // Ensure tick loop exits even when channel closes unexpectedly.
            let mut guard = emitter_for_cmd.lock().await;
            guard.finish();
        }));

        Self {
            cmd_tx: std::sync::Mutex::new(Some(tx)),
            emitter_handle,
            cmd_handle,
            utterance_callback: None,
            vad_start_callback: None,
            vad_end_callback: None,
            vad_start_emitted: std::sync::atomic::AtomicBool::new(false),
            session_state: std::sync::Mutex::new(TranscriptReducer::default()),
            delta_render_mode: DeltaRenderMode::SessionRendered,
            transcript_bus,
        }
    }

    /// Install the per-utterance delivery callback (Toggle mode). Called once
    /// per committed utterance with the text that reached the canvas.
    pub fn set_utterance_callback(&mut self, cb: Option<Arc<dyn Fn(String) + Send + Sync>>) {
        self.utterance_callback = cb;
    }

    /// Choose whether the delta sink sees the whole session or only the live
    /// preview. See [`DeltaRenderMode`].
    pub fn set_delta_render_mode(&mut self, mode: DeltaRenderMode) {
        self.delta_render_mode = mode;
    }

    /// Install the speech-start callback. Fired once per speech run — the
    /// emitter de-duplicates repeated `VadStart` events until a `VadEnd`
    /// re-arms it.
    pub fn set_vad_start_callback(&mut self, cb: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.vad_start_callback = cb;
    }

    /// Install the silence-boundary callback, fired on every `VadEnd`.
    pub fn set_vad_end_callback(&mut self, cb: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.vad_end_callback = cb;
    }

    /// Signal the emitter to finish and wait for both the command worker
    /// and the tick loop to complete.
    pub async fn finish(&mut self) {
        // Send Finish through channel (ordered after all pending pushes).
        if let Ok(guard) = self.cmd_tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(EmitterCmd::Finish);
        }

        // Wait for command worker to drain and exit.
        if let Some(handle) = self.cmd_handle.take()
            && let Err(e) = handle.await
        {
            tracing::error!("Emitter cmd worker failed: {}", e);
        }

        // Wait for tick loop to finish.
        if let Some(handle) = self.emitter_handle.take()
            && let Err(e) = handle.await
        {
            tracing::error!("Emitter tick loop failed: {}", e);
        }
    }

    /// Send a command to the emitter worker (non-blocking, ordered).
    fn send_cmd(&self, cmd: EmitterCmd) {
        if let Ok(guard) = self.cmd_tx.lock()
            && let Some(tx) = guard.as_ref()
            && tx.send(cmd).is_err()
        {
            debug!("Emitter channel closed, dropping command");
        }
    }
}

impl Drop for PresentationEmitter {
    /// Close the cmd channel and abort emitter worker tasks to avoid leaks.
    fn drop(&mut self) {
        // Close command channel first (lets cmd worker exit naturally).
        if let Ok(mut guard) = self.cmd_tx.lock() {
            let _ = guard.take();
        }
        // Abort detached tasks as a hard stop fallback to avoid leaks.
        if let Some(handle) = self.cmd_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.emitter_handle.take() {
            handle.abort();
        }
    }
}

impl EventSink for PresentationEmitter {
    /// Route an `EngineEvent` into session state and the buffered typing emitter.
    fn on_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::VadStart { .. } => {
                if !self
                    .vad_start_emitted
                    .swap(true, std::sync::atomic::Ordering::SeqCst)
                    && let Some(cb) = &self.vad_start_callback
                {
                    cb();
                }
            }
            EngineEvent::VadEnd { .. } => {
                self.vad_start_emitted
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                if let Some(cb) = &self.vad_end_callback {
                    cb();
                }
            }
            EngineEvent::SidebandEvidence { evidence } => {
                debug!(
                    sequence = evidence.sequence,
                    sample_start = evidence.range.sample_start,
                    sample_end = evidence.range.sample_end,
                    "PresentationEmitter observed sideband evidence without mutating text"
                );
            }
            EngineEvent::Preview { .. } => {
                let rendered = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = state.apply_event(event);
                    match self.delta_render_mode {
                        DeltaRenderMode::SessionRendered => state.rendered_text(),
                        DeltaRenderMode::ActivePreviewOnly => state.active_preview.clone(),
                    }
                };
                self.send_cmd(EmitterCmd::SetTargetText(rendered));
            }
            EngineEvent::Correction {
                text,
                previous_text,
                ..
            } => {
                let (rendered, revised) = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    let revised = state
                        .apply_correction(previous_text, text)
                        .and_then(|index| state.committed.get(index))
                        .map(TranscriptUtteranceRecord::clean_draft);
                    let rendered = match self.delta_render_mode {
                        DeltaRenderMode::SessionRendered => state.rendered_text(),
                        DeltaRenderMode::ActivePreviewOnly => state.active_preview.clone(),
                    };
                    (rendered, revised)
                };
                if let (Some(bus), Some(revised)) = (&self.transcript_bus, revised) {
                    bus.publish_draft(TranscriptDraftStatus::Revised, revised);
                }
                self.send_cmd(EmitterCmd::SetTargetText(rendered));
            }
            EngineEvent::UtteranceFinal { utterance_id, .. } => {
                let (callback_payload, committed, revised) = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    let existed = state
                        .committed
                        .iter()
                        .any(|record| record.utterance_id == *utterance_id);
                    let callback_payload = state.apply_event(event);
                    let committed = state
                        .committed
                        .iter()
                        .rfind(|record| record.utterance_id == *utterance_id)
                        .map(TranscriptUtteranceRecord::clean_draft);
                    (callback_payload, committed, existed)
                };
                if let (Some(bus), Some(committed)) = (&self.transcript_bus, committed) {
                    bus.publish_draft(
                        if revised {
                            TranscriptDraftStatus::Revised
                        } else {
                            TranscriptDraftStatus::Created
                        },
                        committed,
                    );
                }
                if let Some(cb) = &self.utterance_callback
                    && let Some(payload) = callback_payload
                {
                    cb(payload);
                }
                if matches!(self.delta_render_mode, DeltaRenderMode::SessionRendered) {
                    let (rendered, committed_len) = {
                        let state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                        (state.rendered_text(), state.committed.len())
                    };
                    // Diagnostic (ADR 2026-05-28 Faza 1 append regression): the emitter
                    // cadence is unit-proven cumulative (session_rendered_accumulates_across_
                    // multiple_utterances), so if a LIVE hands-off session still shows replace,
                    // the cause is upstream — either UtteranceFinal never reaching here during
                    // continuous speech, or the emitter being recreated mid-session. This
                    // per-utterance (low-frequency) info! confirms at runtime whether
                    // `committed` actually grows. info! so it survives release tracing level.
                    info!(
                        utterance_id = *utterance_id,
                        committed_utterances = committed_len,
                        rendered_chars = rendered.chars().count(),
                        "PresentationEmitter: utterance committed (session-rendered, cumulative)"
                    );
                    self.send_cmd(EmitterCmd::SetTargetText(rendered));
                }
            }
            EngineEvent::NoSpeech { reason } => {
                let rendered = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = state.apply_event(event);
                    state.rendered_text()
                };
                self.send_cmd(EmitterCmd::SetTargetText(rendered));
                info!("Engine reported no speech: {}", reason);
            }
            EngineEvent::Drop { kind, text, reason } => {
                debug!(
                    "Engine dropped: {:?} — {} (text: '{}')",
                    kind,
                    reason,
                    text.chars().take(50).collect::<String>()
                );
            }
            EngineEvent::Stats {
                hallucination_drops,
                semantic_gate_drops,
                filtered_empty_drops,
                corrections_applied,
                total_utterances,
                dropped_audio_chunks,
                partial_runs_total,
                trigger_utterance_count,
                trigger_speech_count,
                trigger_timer_count,
                partial_stale_count,
                partial_coalesced_count,
                partial_dropped_count,
            } => {
                info!(
                    "Session stats: utterances={}, hallucinations={}, semantic_gate={}, filtered_empty={}, corrections={}, dropped_chunks={}, partial_runs={} (utterance={}, speech={}, watchdog={}, stale={}, coalesced={}, dropped={})",
                    total_utterances,
                    hallucination_drops,
                    semantic_gate_drops,
                    filtered_empty_drops,
                    corrections_applied,
                    dropped_audio_chunks,
                    partial_runs_total,
                    trigger_utterance_count,
                    trigger_speech_count,
                    trigger_timer_count,
                    partial_stale_count,
                    partial_coalesced_count,
                    partial_dropped_count,
                );
                let rendered = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    if !state.committed.is_empty() {
                        // Session shutdown should not leave an uncommitted preview tail
                        // visible after finalized utterances have already been appended.
                        state.clear_live_preview();
                    }
                    state.rendered_text()
                };
                self.send_cmd(EmitterCmd::SetTargetText(rendered));
                // Stats is the last event from transcription_session.
                // Signal BufferedEmitter to finish through the ordered channel,
                // ensuring all pending pushes are processed first.
                self.send_cmd(EmitterCmd::Finish);
            }
            EngineEvent::Warning { code, message } => {
                tracing::warn!("Engine warning [{}]: {}", code, message);
            }
            EngineEvent::ReplaceRange { .. } | EngineEvent::InsertAnnotation { .. } => {
                // Apply the same bounded correction to the authoritative buffer
                // (transcript_buffer → paste/history) that the overlay already
                // received, so phase-1 layered patches don't diverge between the
                // two sinks. Only re-render when the buffer actually changed.
                let (rendered, revised) = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.apply_layered_patch(event) {
                        let rendered = Some(match self.delta_render_mode {
                            DeltaRenderMode::SessionRendered => state.rendered_text(),
                            DeltaRenderMode::ActivePreviewOnly => state.active_preview.clone(),
                        });
                        let utterance_id = match event {
                            EngineEvent::ReplaceRange { utterance_id, .. }
                            | EngineEvent::InsertAnnotation { utterance_id, .. } => *utterance_id,
                            _ => unreachable!(),
                        };
                        let revised = state
                            .committed
                            .iter()
                            .rfind(|record| record.utterance_id == utterance_id)
                            .map(TranscriptUtteranceRecord::clean_draft);
                        (rendered, revised)
                    } else {
                        (None, None)
                    }
                };
                if let (Some(bus), Some(revised)) = (&self.transcript_bus, revised) {
                    bus.publish_draft(TranscriptDraftStatus::Revised, revised);
                }
                if let Some(rendered) = rendered {
                    self.send_cmd(EmitterCmd::SetTargetText(rendered));
                }
            }
            EngineEvent::SessionFinalised { .. } => {
                // The Apple progressive lane closes with SessionFinalised and
                // does not emit Stats. Persist only immutable canvas here: a
                // cumulative final can re-state committed text as the last
                // Preview, and ignoring the close event would deliver
                // `committed + restatement` at stop.
                let rendered = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.clear_live_preview();
                    state.streaming_floor()
                };
                // Engine close is not product truth. The controller can still
                // run Smart/Always final pass, adjudication, postprocess, and
                // formatting. Only that controller result may seal the bus.
                self.send_cmd(EmitterCmd::SetTargetText(rendered));
                self.send_cmd(EmitterCmd::Finish);
            }
        }
    }
}

/// Session canvas, correction, and delivery-buffer presentation tests.
#[cfg(test)]
mod tests {
    use super::{DeltaRenderMode, PresentationEmitter, SessionTranscriptState};
    use codescribe_core::pipeline::contracts::{
        AnnotationKind, EngineEvent, EventSink, LayerSource, LayerSummary, NonSpeechEvidence,
        SidebandEvidence, SidebandEvidenceKind, SidebandProvenance, TranscriptSegment,
    };
    use codescribe_core::stt::tail_provider::TailSampleRange;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    /// Regression for the 2026-08-14 patch/restatement race.
    ///
    /// Layer 1 computes a recovery against the canvas as it stood when the job
    /// was dispatched. SFSpeech may then restate the SAME utterance at greater
    /// length and deliver those words itself. Measured on take 144425: the
    /// append was computed for a 15-character canvas, the final arrived at 47
    /// characters carrying the phrase, and applying the patch duplicated it
    /// ("…hard pruna I road która pozwoli nam na zrobienie hard Pru.") — three
    /// repeated 4-grams, WER 0.463 → 0.610. The reducer is the last place that
    /// sees the canvas as it actually stands, so the guard belongs here.
    #[test]
    fn patch_already_delivered_by_a_restatement_is_dropped() {
        let mut reducer = SessionTranscriptState::default();
        reducer.apply_event(&EngineEvent::UtteranceFinal {
            utterance_id: 6,
            text: "I road która pozwoli nam na zrobienie hard Pru.".to_string(),
            raw_text: "i road ktora pozwoli nam na zrobienie hard pru".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        let before = reducer.rendered_text();

        // The patch Layer 1 computed against the earlier, shorter canvas.
        reducer.apply_event(&EngineEvent::ReplaceRange {
            utterance_id: 6,
            start: 5,
            end: 5,
            text: " która pozwoli nam na zrobienie hard pruna".to_string(),
            source: LayerSource::TailPatch,
        });
        assert_eq!(
            reducer.rendered_text(),
            before,
            "a recovery the restatement already delivered must not be applied twice"
        );

        // A genuine gap fill on the same utterance still lands.
        reducer.apply_event(&EngineEvent::ReplaceRange {
            utterance_id: 6,
            start: 46,
            end: 46,
            text: " przed wydaniem".to_string(),
            source: LayerSource::TailPatch,
        });
        assert!(
            reducer.rendered_text().contains("przed wydaniem"),
            "novel recovered material must still reach the canvas: {:?}",
            reducer.rendered_text()
        );
    }

    #[test]
    fn sideband_evidence_is_byte_stable_in_the_transcript_reducer() {
        let mut reducer = SessionTranscriptState::default();
        reducer.apply_event(&EngineEvent::UtteranceFinal {
            utterance_id: 11,
            text: "Zażółć gęślą jaźń.".to_string(),
            raw_text: "Zażółć gęślą jaźń.".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        let before = reducer.rendered_text();

        let callback = reducer.apply_event(&EngineEvent::SidebandEvidence {
            evidence: SidebandEvidence {
                sequence: 3,
                range: TailSampleRange {
                    session: "s".to_string(),
                    capture_epoch: 0,
                    sample_start: 16_000,
                    sample_end: 24_000,
                },
                sample_rate_hz: 16_000,
                provenance: SidebandProvenance::SileroVad,
                evidence: SidebandEvidenceKind::Pause {
                    duration_samples: 8_000,
                    non_speech: NonSpeechEvidence::UnknownNonSpeech,
                },
            },
        });

        assert!(callback.is_none());
        assert_eq!(reducer.rendered_text().as_bytes(), before.as_bytes());
    }

    /// Regression for the 2026-08-14 tripled-RAW incident (Monika's take:
    /// reducer said 228 chars, the RAW pulled by `recorder.stop()` said 791).
    /// Two writers raced on the shared buffer: the command worker snapshotted
    /// the full target AND the tick loop appended the same suffix again, so
    /// cumulative Apple previews multiplied the trailing sentence.
    ///
    /// This walks the exact runtime seam — `on_event` → reducer → command
    /// channel → worker snapshot → tick animation → shared buffer — with the
    /// only substituted boundary being the event source, and demands the buffer
    /// end byte-identical to the reducer truth.
    #[tokio::test]
    async fn transcript_buffer_matches_reducer_truth_after_cumulative_previews() {
        let transcript = Arc::new(Mutex::new(String::new()));
        let mut emitter = PresentationEmitter::new(transcript.clone(), None, None);

        let final_event = |id: u64, text: &str, start: f32, end: f32| EngineEvent::UtteranceFinal {
            utterance_id: id,
            text: text.to_string(),
            raw_text: text.to_string(),
            start_ts: start,
            end_ts: end,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        };

        // The Apple-lane shape from the incident log: per-utterance previews
        // grow until a final seals them (the restated-prefix guards upstream
        // strip whole-session restatements before emission), and stop arrives
        // with an open partial still on the canvas — sealed=2 + open tail.
        let events = vec![
            EngineEvent::Preview {
                rev: 1,
                text: "Pies od wczoraj".to_string(),
            },
            EngineEvent::Preview {
                rev: 2,
                text: "Pies od wczoraj wymiotuje.".to_string(),
            },
            final_event(1, "Pies od wczoraj wymiotuje.", 0.0, 2.0),
            EngineEvent::Preview {
                rev: 3,
                text: "Nie je i nie".to_string(),
            },
            EngineEvent::Preview {
                rev: 4,
                text: "Nie je i nie pije.".to_string(),
            },
            final_event(2, "Nie je i nie pije.", 2.0, 4.0),
            EngineEvent::Preview {
                rev: 5,
                text: "Podałam mu".to_string(),
            },
        ];

        let mut reference = SessionTranscriptState::default();
        for event in &events {
            emitter.on_event(event);
            let _ = reference.apply_event(event);
        }
        emitter.finish().await;

        let raw = transcript.lock().await.clone();
        assert_eq!(
            raw,
            reference.rendered_text(),
            "the RAW buffer recorder.stop() reads must be byte-identical to the reducer truth"
        );
        assert_eq!(
            raw.matches("wymiotuje").count(),
            1,
            "a sentence delivered once must appear exactly once in the RAW, got: {raw:?}"
        );
    }

    /// Live preview appends after committed text in the rendered session canvas.
    #[test]
    fn session_state_appends_preview_after_committed_text() {
        let mut state = SessionTranscriptState::default();
        let committed = state.finalize(
            1,
            "Pierwszy fragment",
            "Pierwszy fragment",
            0.0,
            1.0,
            Vec::new(),
        );
        assert_eq!(committed.as_deref(), Some("Pierwszy fragment"));

        state.apply_preview("drugi partial");

        assert_eq!(state.rendered_text(), "Pierwszy fragment drugi partial");
    }

    /// ReplaceRange patches land in the authoritative committed paste buffer.
    #[test]
    fn replace_range_patches_committed_utterance_in_authoritative_buffer() {
        // A phase-1 ReplaceRange fixing "wrold"→"world" must land in the
        // committed (paste/history) buffer, not just the overlay.
        let mut state = SessionTranscriptState::default();
        state.finalize(1, "hello wrold", "hello wrold", 0.0, 1.0, Vec::new());
        let event = EngineEvent::ReplaceRange {
            utterance_id: 1,
            start: 6,
            end: 11,
            text: "world".to_string(),
            source: LayerSource::TailPatch,
        };
        assert!(state.apply_layered_patch(&event));
        assert_eq!(state.rendered_text(), "hello world");
    }

    /// InsertAnnotation appends annotation text into the committed utterance.
    #[test]
    fn insert_annotation_lands_in_committed_utterance() {
        let mut state = SessionTranscriptState::default();
        state.finalize(2, "yes", "yes", 0.0, 1.0, Vec::new());
        let event = EngineEvent::InsertAnnotation {
            utterance_id: 2,
            position: 3,
            text: " [pauza]".to_string(),
            kind: AnnotationKind::HesitationPause,
        };
        assert!(state.apply_layered_patch(&event));
        assert_eq!(state.rendered_text(), "yes [pauza]");
    }

    /// Patches for unknown utterance ids are dropped, not applied elsewhere.
    #[test]
    fn patch_for_uncommitted_utterance_is_ignored() {
        // Offsets reference an utterance the authoritative buffer has not
        // committed yet — drop the patch instead of corrupting another one.
        let mut state = SessionTranscriptState::default();
        state.finalize(1, "hello", "hello", 0.0, 1.0, Vec::new());
        let event = EngineEvent::ReplaceRange {
            utterance_id: 99,
            start: 0,
            end: 1,
            text: "X".to_string(),
            source: LayerSource::Lexicon,
        };
        assert!(!state.apply_layered_patch(&event));
        assert_eq!(state.rendered_text(), "hello");
    }

    /// Active-tail corrections rewrite only the live preview, not prior commits.
    #[test]
    fn session_state_correction_stays_local_to_active_tail() {
        let mut state = SessionTranscriptState::default();
        let _ = state.finalize(
            1,
            "Pierwszy fragment",
            "Pierwszy fragment",
            0.0,
            1.0,
            Vec::new(),
        );
        state.apply_preview("drugi parcjal");
        state.apply_correction("drugi parcjal", "drugi partial");

        assert_eq!(state.rendered_text(), "Pierwszy fragment drugi partial");
    }

    /// Backspace trims the live preview without mutating committed utterances.
    #[test]
    fn session_state_backspace_only_touches_active_preview() {
        let mut state = SessionTranscriptState::default();
        let _ = state.finalize(
            1,
            "Pierwszy fragment",
            "Pierwszy fragment",
            0.0,
            1.0,
            Vec::new(),
        );
        state.apply_preview("drugi partial");
        state.backspace_active_preview(3);

        assert_eq!(state.rendered_text(), "Pierwszy fragment drugi part");
    }

    /// Finalize stores utterance id, segment timestamps, and timing metadata.
    #[test]
    fn session_state_preserves_timestamp_metadata() {
        let mut state = SessionTranscriptState::default();
        let segments = vec![
            TranscriptSegment {
                text: "Pierwszy".to_string(),
                start_ts: 0.0,
                end_ts: 0.5,
            },
            TranscriptSegment {
                text: "fragment".to_string(),
                start_ts: 0.5,
                end_ts: 1.0,
            },
        ];

        let payload = state.finalize(
            7,
            "Pierwszy fragment",
            "Pierwszy fragment",
            12.0,
            13.0,
            segments.clone(),
        );

        assert_eq!(payload.as_deref(), Some("Pierwszy fragment"));
        let committed = state.committed();
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].utterance_id, 7);
        assert_eq!(committed[0].start_ts, 12.0);
        assert_eq!(committed[0].end_ts, 13.0);
        assert_eq!(committed[0].segments, segments);
    }

    /// Whitespace-only preview fragments leave the canvas empty.
    #[test]
    fn session_state_ignores_empty_preview_fragment() {
        let mut state = SessionTranscriptState::default();
        state.apply_preview("   ");
        assert!(state.rendered_text().is_empty());
    }

    /// Late correction after finalize patches the commit instead of appending.
    #[test]
    fn correction_after_final_patches_committed_utterance_without_appending() {
        let mut state = SessionTranscriptState::default();
        state.apply_preview("raw words");
        assert_eq!(
            state.finalize(1, "raw words", "raw words", 0.0, 1.0, Vec::new()),
            Some("raw words".to_string())
        );

        state.apply_correction("raw words", "corrected words");

        assert_eq!(state.rendered_text(), "corrected words");
        assert_eq!(state.committed().len(), 1);
        assert_eq!(state.committed()[0].text, "corrected words");
        assert!(state.active_preview.is_empty());
    }

    /// Correction-after-final yields one delivery buffer utterance, not two.
    #[tokio::test]
    async fn delivery_buffer_receives_one_utterance_when_correction_finishes_after_final() {
        let transcript = Arc::new(Mutex::new(String::new()));
        let emitter = PresentationEmitter::new(transcript.clone(), None, None);

        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "raw words".to_string(),
        });
        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "raw words".to_string(),
            raw_text: "raw words".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        emitter.on_event(&EngineEvent::Correction {
            rev: 2,
            text: "corrected words".to_string(),
            previous_text: "raw words".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        assert_eq!(transcript.lock().await.as_str(), "corrected words");
    }

    /// Correction on a new tail still appends after a prior committed utterance.
    #[tokio::test]
    async fn correction_after_final_still_appends_after_previous_utterance() {
        let transcript = Arc::new(Mutex::new(String::new()));
        let emitter = PresentationEmitter::new(transcript.clone(), None, None);

        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "Ala ma".to_string(),
        });
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;

        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "Ala ma".to_string(),
            raw_text: "Ala ma".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        emitter.on_event(&EngineEvent::Preview {
            rev: 2,
            text: "koc".to_string(),
        });
        emitter.on_event(&EngineEvent::Correction {
            rev: 3,
            text: "kota".to_string(),
            previous_text: "koc".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let snapshot = transcript.lock().await.clone();
        assert!(
            snapshot.contains("Ala ma kota"),
            "expected correction to survive utterance boundary, got: {snapshot:?}"
        );
        assert!(
            snapshot.starts_with("Ala ma"),
            "expected previous utterance to stay committed, got: {snapshot:?}"
        );
    }

    /// Multi-utterance sessions accumulate append-only in SessionRendered mode.
    #[tokio::test]
    async fn session_rendered_accumulates_across_multiple_utterances() {
        // ADR 2026-05-28 Faza 1: hands-off long-form must build ONE continuous
        // transcript — every finalized utterance APPENDS, never replaces. This drives
        // a realistic multi-utterance cadence (Preview -> UtteranceFinal x3, plus a
        // trailing live preview) through the default SessionRendered mode and asserts
        // the rendered buffer is cumulative. Guards the operator-reported regression
        // "UI nie dodaje tekstu na końcu ogona" (replace instead of append).
        let transcript = Arc::new(Mutex::new(String::new()));
        let emitter = PresentationEmitter::new(transcript.clone(), None, None);

        let finalize = |id: u64, text: &str| EngineEvent::UtteranceFinal {
            utterance_id: id,
            text: text.to_string(),
            raw_text: text.to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        };

        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "Pierwsze".to_string(),
        });
        emitter.on_event(&finalize(1, "Pierwsze zdanie."));
        emitter.on_event(&EngineEvent::Preview {
            rev: 2,
            text: "drugie".to_string(),
        });
        emitter.on_event(&finalize(2, "drugie zdanie."));
        emitter.on_event(&EngineEvent::Preview {
            rev: 3,
            text: "trzecie na żywo".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let snapshot = transcript.lock().await.clone();
        assert!(
            snapshot.contains("Pierwsze zdanie.")
                && snapshot.contains("drugie zdanie.")
                && snapshot.contains("trzecie na żywo"),
            "session-rendered must accumulate every utterance (append, not replace), got: {snapshot:?}"
        );
    }

    /// Empty final falls back to last preview; duplicate utterance ids dedupe.
    #[tokio::test]
    async fn utterance_callback_falls_back_to_last_preview_and_dedupes() {
        let transcript = Arc::new(Mutex::new(String::new()));
        let mut emitter = PresentationEmitter::new(transcript, None, None);
        let delivered = Arc::new(StdMutex::new(Vec::<String>::new()));
        let delivered_ref = Arc::clone(&delivered);
        emitter.set_utterance_callback(Some(Arc::new(move |text: String| {
            delivered_ref
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(text);
        })));

        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "ostatni sensowny preview".to_string(),
        });
        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 7,
            text: "   ".to_string(),
            raw_text: String::new(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(5.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 7,
            text: "duplikat".to_string(),
            raw_text: "duplikat".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(5.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });

        let delivered = delivered.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            delivered,
            vec!["ostatni sensowny preview".to_string()],
            "empty final should fallback to preview and duplicate utterance must be ignored"
        );
    }

    /// ActivePreviewOnly streams only the live tail, never prior commits.
    #[tokio::test]
    async fn active_preview_only_mode_does_not_carry_previous_utterance_into_next_preview() {
        let transcript = Arc::new(Mutex::new(String::new()));
        let mut emitter = PresentationEmitter::new(transcript.clone(), None, None);
        emitter.set_delta_render_mode(DeltaRenderMode::ActivePreviewOnly);

        emitter.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "pierwszy utterance".to_string(),
        });
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;

        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "pierwszy utterance".to_string(),
            raw_text: "pierwszy utterance".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        emitter.on_event(&EngineEvent::Preview {
            rev: 2,
            text: "drugi fragment".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let snapshot = transcript.lock().await.clone();
        assert_eq!(
            snapshot, "drugi fragment",
            "assistive preview should stream only the live utterance, got: {snapshot:?}"
        );
    }

    /// Stats event clears a dangling uncommitted preview after finalize.
    #[tokio::test]
    async fn stats_clears_uncommitted_preview_after_finalized_utterance() {
        let transcript = Arc::new(Mutex::new(String::new()));
        let emitter = PresentationEmitter::new(transcript.clone(), None, None);

        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "Ala ma kota".to_string(),
            raw_text: "Ala ma kota".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        emitter.on_event(&EngineEvent::Preview {
            rev: 2,
            text: "śmieciowy ogon".to_string(),
        });
        emitter.on_event(&EngineEvent::Stats {
            dropped_audio_chunks: 0,
            hallucination_drops: 0,
            semantic_gate_drops: 0,
            filtered_empty_drops: 0,
            corrections_applied: 0,
            total_utterances: 1,
            partial_runs_total: 0,
            trigger_utterance_count: 0,
            trigger_speech_count: 0,
            trigger_timer_count: 0,
            partial_stale_count: 0,
            partial_coalesced_count: 0,
            partial_dropped_count: 0,
        });

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let snapshot = transcript.lock().await.clone();
        assert_eq!(snapshot, "Ala ma kota");
    }

    /// Apple progressive closes with `SessionFinalised`, not `Stats`. A fully
    /// re-heard cumulative final can leave the committed canvas in Preview;
    /// ignoring the closing event then persists `committed + restatement`.
    #[tokio::test]
    async fn session_finalised_clears_reheard_preview_without_stats() {
        let transcript = Arc::new(Mutex::new(String::new()));
        let mut emitter = PresentationEmitter::new(transcript.clone(), None, None);

        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "Ala ma kota".to_string(),
            raw_text: "Ala ma kota".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        emitter.on_event(&EngineEvent::Preview {
            rev: 2,
            text: "Ala ma kota".to_string(),
        });
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: "session".to_string(),
            layer_summary: LayerSummary::default(),
        });
        emitter.finish().await;

        let snapshot = transcript.lock().await.clone();
        assert_eq!(
            snapshot, "Ala ma kota",
            "SessionFinalised must persist committed canvas only"
        );
    }

    /// Late correction matching penultimate commit patches it, never appends.
    #[tokio::test]
    async fn correction_targets_penultimate_utterance_patches_instead_of_appending() {
        // P3-03 over-correct + marbles fortify: late correction whose previous_text
        // matches a non-tail (penultimate) committed utterance must patch it, not
        // append via preview fallback. This closes the "korekta do przedostatniej
        // wypowiedzi appenduje" gap.
        let transcript = Arc::new(Mutex::new(String::new()));
        let emitter = PresentationEmitter::new(transcript.clone(), None, None);

        // Commit two utterances.
        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "Ala ma kota.".to_string(),
            raw_text: "Ala ma kota.".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 2,
            text: "A kot ma Ale.".to_string(),
            raw_text: "A kot ma Ale.".to_string(),
            start_ts: 0.0,
            end_ts: 2.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });

        // Late correction targets the *first* (penultimate at arrival) utterance.
        emitter.on_event(&EngineEvent::Correction {
            rev: 99,
            text: "Ala ma psa.".to_string(),
            previous_text: "Ala ma kota.".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let snapshot = transcript.lock().await.clone();
        assert!(
            snapshot.contains("Ala ma psa."),
            "penultimate correction must patch in place, got: {snapshot:?}"
        );
        assert!(
            snapshot.contains("A kot ma Ale."),
            "later utterance must remain untouched, got: {snapshot:?}"
        );
        // No duplication of the corrected text.
        assert_eq!(snapshot.matches("Ala ma").count(), 1);
    }

    /// Dictation and Agent differ only in metadata/consumer choice. The exact
    /// same engine fixture must produce byte-equivalent draft events. The
    /// controller-owned product seal is simulated explicitly after engine close.
    #[tokio::test]
    async fn dictation_and_agent_publish_identical_drafts_before_controller_seal() {
        use crate::presentation::transcript_bus::{
            CleanTranscriptEvent, TranscriptBus, TranscriptMode, TranscriptSession,
        };

        fn run_route(
            root: &std::path::Path,
            mode: TranscriptMode,
            session_id: &str,
        ) -> Vec<CleanTranscriptEvent> {
            let path = root.join(format!("{mode:?}.jsonl"));
            let bus = Arc::new(
                TranscriptBus::open_at(
                    TranscriptSession {
                        session_id: session_id.to_string(),
                        mode,
                    },
                    path.clone(),
                    Some(48_000),
                )
                .unwrap(),
            );
            let transcript = Arc::new(Mutex::new(String::new()));
            let emitter = PresentationEmitter::new_with_transcript_bus(
                transcript,
                None,
                None,
                Some(Arc::clone(&bus)),
            );
            emitter.on_event(&EngineEvent::Preview {
                rev: 1,
                text: "shared clean truth".to_string(),
            });
            emitter.on_event(&EngineEvent::UtteranceFinal {
                utterance_id: 42,
                text: "shared clean truth".to_string(),
                raw_text: "unpublished raw hypothesis".to_string(),
                start_ts: 0.25,
                end_ts: 1.5,
                segments: vec![TranscriptSegment {
                    text: "shared clean truth".to_string(),
                    start_ts: 0.25,
                    end_ts: 1.5,
                }],
                vad_speech_pct: Some(91.0),
                avg_logprob: Some(-0.2),
                compression_ratio: None,
                quality_gate_dropped: false,
                confidence_flags: Vec::new(),
            });
            emitter.on_event(&EngineEvent::SessionFinalised {
                session_id: format!("pipeline-{session_id}"),
                layer_summary: LayerSummary::default(),
            });
            bus.publish_sealed(
                "shared clean truth".to_string(),
                Some(format!("pipeline-{session_id}")),
            );

            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }

        let temp = tempfile::tempdir().unwrap();
        let dictation = run_route(temp.path(), TranscriptMode::Dictation, "dictation-session");
        let agent = run_route(temp.path(), TranscriptMode::Agent, "agent-session");

        let comparable = |events: &[CleanTranscriptEvent]| {
            events
                .iter()
                .skip(1)
                .map(|event| {
                    (
                        event.status.clone(),
                        event.utterance_id,
                        event.sample_rate_hz,
                        event.sample_start,
                        event.sample_end,
                        event.audio_start_seconds,
                        event.audio_end_seconds,
                        event.text.clone(),
                        event.segments.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(comparable(&dictation), comparable(&agent));
        assert_eq!(agent[1].status, "utterance_draft");
        assert_eq!(agent[2].status, "transcript_sealed");
        assert!(
            !agent
                .iter()
                .any(|event| event.text.contains("unpublished raw"))
        );
    }
}
