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

use crate::pipeline::contracts::{
    DropKind, EngineEvent, EventSink, LayerSummary, TranscriptSegment,
};
use crate::pipeline::stream_postprocess::StreamPostProcessor;
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
        Ok(Ok(outcome)) => {
            info!(
                sealed = outcome.sealed,
                filtered_empty_drops = outcome.filtered_empty_drops,
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

    event_sink.on_event(&EngineEvent::SessionFinalised {
        session_id,
        layer_summary: LayerSummary::default(),
    });
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
}

impl AppleSealState {
    fn new() -> Self {
        Self {
            postprocessor: StreamPostProcessor::new(),
            preview_rev: 0,
            utterance_id: 0,
            open_partial: String::new(),
            sealed_count: 0,
            filtered_empty_drops: 0,
        }
    }
}

/// What the worker sealed, and what seal-time postprocess filtered away.
struct AppleStreamOutcome {
    sealed: u64,
    filtered_empty_drops: u64,
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
    let text = corrected.trim().to_string();
    state.utterance_id = state.utterance_id.saturating_add(1);
    state.sealed_count = state.sealed_count.saturating_add(1);
    let _ = ev_tx.send(EngineEvent::UtteranceFinal {
        utterance_id: state.utterance_id,
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
    true
}

/// Blocking worker: owns the stream session for its full lifetime.
fn apple_stream_worker(
    pcm_rx: std_mpsc::Receiver<Option<Vec<f32>>>,
    ev_tx: mpsc::UnboundedSender<EngineEvent>,
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<AppleStreamOutcome> {
    let mut stream = LiveStreamSession::open(language, sample_rate)?;
    let mut state = AppleSealState::new();
    let mut samples_seen: u64 = 0;

    loop {
        // Interleave PCM wait with progressive event polling so partials land
        // mid-utterance without waiting for the next audio chunk.
        match pcm_rx.recv_timeout(Duration::from_millis(40)) {
            Ok(Some(samples)) => {
                samples_seen += samples.len() as u64;
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
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::apple_stt::parse_stream_stdout_line;

    #[test]
    fn emit_maps_partial_and_two_phrase_finals() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new();
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

    /// W1-A contract: lexicon correction must land at SEAL time on the Apple
    /// progressive path — before the text becomes committed canvas. The Apple
    /// path used to emit raw SFSpeech text and rely on the stop-path
    /// postprocess, which is a post-commit rewrite (forbidden by the
    /// append-only doctrine).
    #[test]
    fn apple_seal_lexicon_corrects_sealed_final() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new();
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
        assert_eq!(text, "uruchom Docker teraz");
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
        let mut state = AppleSealState::new();
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
        let mut state = AppleSealState::new();
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
        let mut state = AppleSealState::new();
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
        assert_eq!(text, "zbuduj obraz Docker");
        assert!(state.open_partial.is_empty());
    }

    /// The stop-path postprocess still runs over committed text. Seal-time
    /// correction is only append-safe because a second application is a no-op.
    #[test]
    fn apple_seal_lexicon_is_idempotent_under_stop_path_postprocess() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new();
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
