//! Event-driven presentation emitter.
//!
//! Converts `EngineEvent`s into user-facing output by delegating to
//! `BufferedEmitter` (typing animation, delta encoding) from core.
//!
//! Uses an ordered mpsc channel to guarantee that target updates and finish
//! arrive in the exact order they were emitted,
//! eliminating the fire-and-forget tokio::spawn ordering race.
//!
//! Created by M&K (c)2026 VetCoders

use std::sync::Arc;

use codescribe_core::pipeline::contracts::{DeltaSink, EngineEvent, EventSink, TranscriptSegment};
use codescribe_core::pipeline::streaming::BufferedEmitter;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// Commands sent through the ordered channel to the emitter worker.
enum EmitterCmd {
    SetTargetText(String),
    Finish,
}

#[derive(Debug, Clone, PartialEq)]
struct TranscriptUtteranceRecord {
    utterance_id: u64,
    text: String,
    raw_text: String,
    start_ts: f32,
    end_ts: f32,
    segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Default)]
struct SessionTranscriptState {
    committed: Vec<TranscriptUtteranceRecord>,
    active_preview: String,
    last_non_empty_preview: String,
}

fn normalize_transcript_fragment(text: &str) -> String {
    text.trim().to_string()
}

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

impl SessionTranscriptState {
    fn apply_preview(&mut self, text: &str) {
        let normalized = normalize_transcript_fragment(text);
        self.active_preview = normalized.clone();
        if !normalized.is_empty() {
            self.last_non_empty_preview = normalized;
        }
    }

    fn apply_correction(&mut self, text: &str) {
        self.apply_preview(text);
    }

    #[cfg(test)]
    fn backspace_active_preview(&mut self, delete_count: usize) {
        for _ in 0..delete_count {
            self.active_preview.pop();
        }
        if !self.active_preview.is_empty() {
            self.last_non_empty_preview = self.active_preview.clone();
        }
    }

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

    fn clear_live_preview(&mut self) {
        self.active_preview.clear();
        self.last_non_empty_preview.clear();
    }

    fn rendered_text(&self) -> String {
        let mut rendered = String::new();
        for utterance in &self.committed {
            append_rendered_fragment(&mut rendered, &utterance.text);
        }
        append_rendered_fragment(&mut rendered, &self.active_preview);
        rendered
    }

    #[cfg(test)]
    fn committed(&self) -> &[TranscriptUtteranceRecord] {
        &self.committed
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
    /// Source-of-truth transcript state: committed utterances + active preview tail.
    session_state: std::sync::Mutex<SessionTranscriptState>,
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
            vad_start_emitted: std::sync::atomic::AtomicBool::new(false),
            session_state: std::sync::Mutex::new(SessionTranscriptState::default()),
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
                let rendered = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.apply_preview(text);
                    state.rendered_text()
                };
                self.send_cmd(EmitterCmd::SetTargetText(rendered));
            }
            EngineEvent::Correction { text, .. } => {
                let rendered = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.apply_correction(text);
                    state.rendered_text()
                };
                self.send_cmd(EmitterCmd::SetTargetText(rendered));
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
                let (rendered, callback_payload) = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    let payload = state.finalize(
                        *utterance_id,
                        text,
                        raw_text,
                        *start_ts,
                        *end_ts,
                        segments.clone(),
                    );
                    (state.rendered_text(), payload)
                };
                self.send_cmd(EmitterCmd::SetTargetText(rendered));
                if let Some(cb) = &self.utterance_callback
                    && let Some(payload) = callback_payload
                {
                    cb(payload);
                }
            }
            EngineEvent::NoSpeech { reason } => {
                let rendered = {
                    let mut state = self.session_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.clear_live_preview();
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
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PresentationEmitter, SessionTranscriptState};
    use codescribe_core::pipeline::contracts::{EngineEvent, EventSink, TranscriptSegment};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

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
        state.apply_correction("drugi partial");

        assert_eq!(state.rendered_text(), "Pierwszy fragment drugi partial");
    }

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

    #[test]
    fn session_state_ignores_empty_preview_fragment() {
        let mut state = SessionTranscriptState::default();
        state.apply_preview("   ");
        assert!(state.rendered_text().is_empty());
    }

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
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
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
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
        });
        emitter.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 7,
            text: "duplikat".to_string(),
            raw_text: "duplikat".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
        });

        let delivered = delivered.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            delivered,
            vec!["ostatni sensowny preview".to_string()],
            "empty final should fallback to preview and duplicate utterance must be ignored"
        );
    }

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
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
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
            trigger_watchdog_count: 0,
            partial_stale_count: 0,
            partial_coalesced_count: 0,
            partial_dropped_count: 0,
        });

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let snapshot = transcript.lock().await.clone();
        assert_eq!(snapshot, "Ala ma kota");
    }
}
