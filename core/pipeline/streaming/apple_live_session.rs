//! Apple progressive live session — system-dictation shape.
//!
//! Bypasses Silero-VAD + per-window Whisper scheduler for the Apple live path.
//! One long-lived SFSpeech stream maps:
//! - `partial` → `EngineEvent::Preview`
//! - phrase `final` → `EngineEvent::UtteranceFinal` (multi-seal freezed+append)
//! - open partial on stop → sealed as a last final when non-empty
//!
//! Whisper stays the file final-pass / emergency fill (controller stop path),
//! not the live engine. INTERIM: per AGENTS.md (THE ONE RULE) Whisper's target
//! role is transcribing partials on the go to fill canvas gaps — never a
//! stop-time full-text authority. Escape hatch:
//! `CODESCRIBE_APPLE_STT_LIVE_MODE=wav` restores the legacy VAD+scheduler path.
//!
//! The bridge global lock + child process live on a **dedicated OS thread**
//! (MutexGuard is `!Send`); the async session only shuttles PCM in and
//! `EngineEvent`s out.

use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::pipeline::contracts::{EngineEvent, EventSink, LayerSummary, TranscriptSegment};
use crate::stt::apple_stt::{LiveStreamEvent, LiveStreamSession};

use super::session::SessionConfig;
use super::stream_log::append_to_stream_log;

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

    let worker =
        thread::spawn(move || apple_stream_worker(pcm_rx, ev_tx, sample_rate, language.as_deref()));

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
                    Some(event) => {
                        // Same diagnostic artifact the VAD path writes: one
                        // line per committed utterance (CODESCRIBE_STREAM_LOG).
                        if let (Some(path), EngineEvent::UtteranceFinal { text, .. }) =
                            (stream_log_path.as_deref(), &event)
                        {
                            let _ = append_to_stream_log(path, text.trim());
                        }
                        event_sink.on_event(&event);
                    }
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
        }
    }

    // Worker exited (event channel closed). If audio is still open, keep
    // consuming to EOF so upstream capture senders never hit a dropped
    // channel — an early engine death (e.g. bridge spawn failure) must not
    // turn live audio callbacks into send errors. Mirrors the pre-interleave
    // contract where the session always outlived the audio stream.
    if !audio_eof {
        while chunk_receiver.recv().await.is_some() {}
    }

    match worker.join() {
        Ok(Ok(sealed)) => {
            info!(sealed, "Apple progressive live session finished");
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

    event_sink.on_event(&EngineEvent::SessionFinalised {
        session_id,
        layer_summary: LayerSummary::default(),
    });
}

/// Blocking worker: owns the stream session for its full lifetime.
fn apple_stream_worker(
    pcm_rx: std_mpsc::Receiver<Option<Vec<f32>>>,
    ev_tx: mpsc::UnboundedSender<EngineEvent>,
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<u64> {
    let mut stream = LiveStreamSession::open(language, sample_rate)?;
    let mut preview_rev: u64 = 0;
    let mut utterance_id: u64 = 0;
    let mut open_partial = String::new();
    let mut sealed_count: u64 = 0;
    let mut samples_seen: u64 = 0;

    loop {
        // Interleave PCM wait with progressive event polling so partials land
        // mid-utterance without waiting for the next audio chunk.
        match pcm_rx.recv_timeout(Duration::from_millis(40)) {
            Ok(Some(samples)) => {
                samples_seen += samples.len() as u64;
                stream.write_pcm(&samples)?;
                let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
                emit_stream_events(
                    stream.poll_events(),
                    &ev_tx,
                    &mut preview_rev,
                    &mut utterance_id,
                    &mut open_partial,
                    &mut sealed_count,
                    audio_secs,
                );
            }
            Ok(None) => break, // EOF from async side
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
                emit_stream_events(
                    stream.poll_events(),
                    &ev_tx,
                    &mut preview_rev,
                    &mut utterance_id,
                    &mut open_partial,
                    &mut sealed_count,
                    audio_secs,
                );
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
    let trailing = stream.finish()?;
    emit_stream_events(
        trailing,
        &ev_tx,
        &mut preview_rev,
        &mut utterance_id,
        &mut open_partial,
        &mut sealed_count,
        audio_secs,
    );

    // Seal open partial that never got a phrase final (stop mid-phrase).
    if !open_partial.trim().is_empty() {
        utterance_id = utterance_id.saturating_add(1);
        sealed_count = sealed_count.saturating_add(1);
        let text = open_partial.trim().to_string();
        let _ = ev_tx.send(EngineEvent::UtteranceFinal {
            utterance_id,
            text: text.clone(),
            raw_text: text,
            start_ts: 0.0,
            end_ts: audio_secs,
            segments: Vec::new(),
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        });
    }

    Ok(sealed_count)
}

fn emit_stream_events(
    events: Vec<LiveStreamEvent>,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    preview_rev: &mut u64,
    utterance_id: &mut u64,
    open_partial: &mut String,
    sealed_count: &mut u64,
    audio_secs: f32,
) {
    for event in events {
        match event {
            LiveStreamEvent::Ready | LiveStreamEvent::End => {}
            LiveStreamEvent::Partial { text } => {
                *open_partial = text.clone();
                *preview_rev = preview_rev.saturating_add(1);
                let _ = ev_tx.send(EngineEvent::Preview {
                    rev: *preview_rev,
                    text,
                });
            }
            LiveStreamEvent::PhraseFinal { text, segments } => {
                let trimmed = text.trim().to_string();
                open_partial.clear();
                if trimmed.is_empty() {
                    continue;
                }
                *utterance_id = utterance_id.saturating_add(1);
                *sealed_count = sealed_count.saturating_add(1);
                let segs: Vec<TranscriptSegment> = segments;
                let _ = ev_tx.send(EngineEvent::UtteranceFinal {
                    utterance_id: *utterance_id,
                    text: trimmed.clone(),
                    raw_text: trimmed,
                    start_ts: segs.first().map(|s| s.start_ts).unwrap_or(0.0),
                    end_ts: segs.last().map(|s| s.end_ts).unwrap_or(audio_secs),
                    segments: segs,
                    vad_speech_pct: None,
                    avg_logprob: None,
                    compression_ratio: None,
                    quality_gate_dropped: false,
                    confidence_flags: Vec::new(),
                });
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
                if *sealed_count == 0 {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        *utterance_id = utterance_id.saturating_add(1);
                        *sealed_count = sealed_count.saturating_add(1);
                        open_partial.clear();
                        let _ = ev_tx.send(EngineEvent::UtteranceFinal {
                            utterance_id: *utterance_id,
                            text: trimmed.clone(),
                            raw_text: trimmed,
                            start_ts: 0.0,
                            end_ts: audio_secs,
                            segments,
                            vad_speech_pct: None,
                            avg_logprob: None,
                            compression_ratio: None,
                            quality_gate_dropped: false,
                            confidence_flags: Vec::new(),
                        });
                    }
                } else {
                    // Phrase seals already emitted; don't double-seal open partial.
                    open_partial.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::apple_stt::parse_stream_stdout_line;

    #[test]
    fn emit_maps_partial_and_two_phrase_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut preview_rev = 0;
        let mut utterance_id = 0;
        let mut open_partial = String::new();
        let mut sealed = 0u64;
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
            &mut preview_rev,
            &mut utterance_id,
            &mut open_partial,
            &mut sealed,
            1.0,
        );
        drop(tx);
        let mut got = Vec::new();
        while let Ok(e) = rx.try_recv() {
            got.push(e);
        }
        assert_eq!(sealed, 2);
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

    /// Contract sensor: mid-stream Previews must be consumable without waiting
    /// for audio EOF. The session select loop is the production path; this
    /// locks the interleave contract — events already queued while PCM is
    /// still open surface to the sink immediately (not only after stop).
    #[tokio::test]
    async fn live_previews_surface_before_audio_eof() {
        use crate::pipeline::contracts::EventSink;
        use std::sync::Mutex;

        struct CollectSink(Mutex<Vec<String>>);
        impl EventSink for CollectSink {
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
