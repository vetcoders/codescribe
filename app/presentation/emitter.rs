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

#[derive(Debug, PartialEq, Eq)]
enum PreviewUpdate {
    Noop,
    Segment(String),
    Correction,
}

fn preview_update(last_preview: &str, incoming: &str) -> PreviewUpdate {
    if let Some(stripped) = incoming.strip_prefix(last_preview) {
        let suffix = stripped.to_string();
        if suffix.trim().is_empty() {
            PreviewUpdate::Noop
        } else {
            PreviewUpdate::Segment(suffix)
        }
    } else {
        PreviewUpdate::Correction
    }
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
    /// Last non-empty preview text for boundary fallback when final text is empty.
    last_non_empty_preview: std::sync::Mutex<String>,
    /// Last utterance id delivered to callback (guards duplicate boundary commits).
    last_dispatched_utterance_id: std::sync::atomic::AtomicU64,
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
            last_non_empty_preview: std::sync::Mutex::new(String::new()),
            last_dispatched_utterance_id: std::sync::atomic::AtomicU64::new(0),
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
                //
                // If Preview diverges (not a prefix extension), treat it as a
                // replacement path instead of appending the whole preview.
                // This prevents duplicated/garbled overlay text when partial
                // passes rewrite earlier tokens.
                let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
                let previous_len = last.chars().count();
                let update = preview_update(last.as_str(), text);
                *last = text.clone();
                if !text.trim().is_empty() {
                    let mut last_non_empty = self
                        .last_non_empty_preview
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *last_non_empty = text.clone();
                }
                drop(last);

                match update {
                    PreviewUpdate::Noop => {}
                    PreviewUpdate::Segment(new_suffix) => {
                        self.send_cmd(EmitterCmd::PushSegment(new_suffix));
                    }
                    PreviewUpdate::Correction => {
                        debug!(
                            previous_len,
                            incoming_len = text.chars().count(),
                            "Preview diverged from last preview; routing as correction to avoid append corruption"
                        );
                        self.send_cmd(EmitterCmd::PushCorrection(text.clone()));
                    }
                }
            }
            EngineEvent::Correction {
                text,
                previous_text,
                ..
            } => {
                if text.trim().is_empty() {
                    return;
                }
                let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
                let mut last_non_empty = self
                    .last_non_empty_preview
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let baseline = if !last.trim().is_empty() {
                    last.clone()
                } else if !previous_text.trim().is_empty() {
                    previous_text.clone()
                } else {
                    last_non_empty.clone()
                };
                *last = text.clone();
                *last_non_empty = text.clone();
                drop(last_non_empty);
                drop(last);
                if baseline.trim().is_empty() {
                    debug!(
                        "Correction arrived without preview baseline; routing as segment bootstrap"
                    );
                    self.send_cmd(EmitterCmd::PushSegment(text.clone()));
                } else {
                    self.send_cmd(EmitterCmd::PushCorrection(text.clone()));
                }
            }
            EngineEvent::UtteranceFinal {
                utterance_id, text, ..
            } => {
                // Reset last_preview — engine clears accumulated_text on utterance boundary.
                {
                    let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
                    last.clear();
                }
                let fallback_preview = {
                    let mut last_non_empty = self
                        .last_non_empty_preview
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let value = last_non_empty.trim().to_string();
                    last_non_empty.clear();
                    value
                };
                let duplicate = self
                    .last_dispatched_utterance_id
                    .swap(*utterance_id, std::sync::atomic::Ordering::SeqCst)
                    == *utterance_id;
                if duplicate {
                    debug!(
                        utterance_id = *utterance_id,
                        "Ignoring duplicate UtteranceFinal callback dispatch"
                    );
                    return;
                }
                if let Some(cb) = &self.utterance_callback {
                    let payload = if text.trim().is_empty() {
                        fallback_preview.as_str()
                    } else {
                        text.trim()
                    };
                    if !payload.is_empty() {
                        cb(payload.to_string());
                    }
                }
            }
            EngineEvent::NoSpeech { reason } => {
                {
                    let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
                    last.clear();
                }
                {
                    let mut last_non_empty = self
                        .last_non_empty_preview
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    last_non_empty.clear();
                }
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
                trigger_watchdog_count,
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
                    trigger_watchdog_count,
                    partial_stale_count,
                    partial_coalesced_count,
                    partial_dropped_count,
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

#[cfg(test)]
mod tests {
    use super::{PresentationEmitter, PreviewUpdate, preview_update};
    use codescribe_core::pipeline::contracts::{EngineEvent, EventSink};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    #[test]
    fn preview_update_emits_only_new_suffix_for_prefix_growth() {
        assert_eq!(
            preview_update("No dobra", "No dobra ziomeczku"),
            PreviewUpdate::Segment(" ziomeczku".to_string())
        );
    }

    #[test]
    fn preview_update_routes_divergence_to_correction() {
        assert_eq!(
            preview_update("No dobra ziomeczku", "No dobra, ziomeczku"),
            PreviewUpdate::Correction
        );
    }

    #[test]
    fn preview_update_ignores_whitespace_only_suffix() {
        assert_eq!(preview_update("tekst", "tekst "), PreviewUpdate::Noop);
    }

    #[tokio::test]
    async fn correction_after_final_still_updates_live_buffer() {
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
        });
        emitter.on_event(&EngineEvent::Correction {
            rev: 2,
            text: "Ala ma kota".to_string(),
            previous_text: "Ala ma".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let snapshot = transcript.lock().await.clone();
        assert!(
            snapshot.contains("Ala ma kota"),
            "expected correction to survive utterance boundary, got: {snapshot:?}"
        );
    }

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
        });
        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 7,
            text: "duplikat".to_string(),
            raw_text: "duplikat".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
        });

        let delivered = delivered.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            delivered,
            vec!["ostatni sensowny preview".to_string()],
            "empty final should fallback to preview and duplicate utterance must be ignored"
        );
    }
}
