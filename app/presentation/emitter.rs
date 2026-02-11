//! Event-driven presentation emitter.
//!
//! Converts `EngineEvent`s into user-facing output by delegating to
//! `BufferedEmitter` (typing animation, delta encoding) from core.
//!
//! Uses an ordered mpsc channel to guarantee that push_segment,
//! push_correction and finish arrive in the exact order they were emitted,
//! eliminating the fire-and-forget tokio::spawn ordering race.
//!
//! Created by M&K (c)2026 VetCoders

use std::sync::Arc;

use codescribe_core::pipeline::contracts::{DeltaSink, EngineEvent, EventSink};
use codescribe_core::pipeline::streaming::BufferedEmitter;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// Commands sent through the ordered channel to the emitter worker.
enum EmitterCmd {
    PushSegment(String),
    PushCorrection(String),
    Finish,
}

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
    vad_start_emitted: std::sync::atomic::AtomicBool,
    /// Last preview text — used to compute incremental segment for push_segment.
    last_preview: std::sync::Mutex<String>,
}

impl PresentationEmitter {
    pub fn new(
        transcript_buffer: Arc<Mutex<String>>,
        delta_callback: Option<Arc<dyn DeltaSink>>,
        stream_log_path: Option<std::path::PathBuf>,
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
                    EmitterCmd::PushSegment(text) => guard.push_segment(text),
                    EmitterCmd::PushCorrection(text) => guard.push_correction(text),
                    EmitterCmd::Finish => {
                        guard.finish();
                    }
                }));
                if result.is_err() {
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
            vad_start_emitted: std::sync::atomic::AtomicBool::new(false),
            last_preview: std::sync::Mutex::new(String::new()),
        }
    }

    pub fn set_utterance_callback(&mut self, cb: Option<Arc<dyn Fn(String) + Send + Sync>>) {
        self.utterance_callback = cb;
    }

    pub fn set_vad_start_callback(&mut self, cb: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.vad_start_callback = cb;
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
            EngineEvent::Preview { text, .. } => {
                // Compute only the new suffix since last preview and push
                // that as incremental segment to the buffered emitter.
                let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
                let new_suffix = if text.starts_with(last.as_str()) {
                    text[last.len()..].to_string()
                } else {
                    // Text changed structure (shouldn't happen for Preview, but be safe)
                    text.clone()
                };
                *last = text.clone();
                drop(last);

                if !new_suffix.trim().is_empty() {
                    self.send_cmd(EmitterCmd::PushSegment(new_suffix));
                }
            }
            EngineEvent::Correction { text, .. } => {
                let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
                // Ignore stale corrections after UtteranceFinal reset last_preview.
                if last.is_empty() {
                    debug!("Ignoring Correction with empty last_preview (post-final)");
                    return;
                }
                *last = text.clone();
                drop(last);
                self.send_cmd(EmitterCmd::PushCorrection(text.clone()));
            }
            EngineEvent::UtteranceFinal { text, .. } => {
                // Reset last_preview — engine clears accumulated_text on utterance boundary.
                {
                    let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
                    last.clear();
                }
                if let Some(cb) = &self.utterance_callback {
                    let payload = text.trim();
                    if !payload.is_empty() {
                        cb(payload.to_string());
                    }
                }
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
            } => {
                info!(
                    "Session stats: utterances={}, hallucinations={}, semantic_gate={}, filtered_empty={}, corrections={}, dropped_chunks={}",
                    total_utterances,
                    hallucination_drops,
                    semantic_gate_drops,
                    filtered_empty_drops,
                    corrections_applied,
                    dropped_audio_chunks,
                );
                // Stats is the last event from transcription_session.
                // Signal BufferedEmitter to finish through the ordered channel,
                // ensuring all pending pushes are processed first.
                self.send_cmd(EmitterCmd::Finish);
            }
            EngineEvent::Warning { code, message } => {
                tracing::warn!("Engine warning [{}]: {}", code, message);
            }
            _ => {}
        }
    }
}
