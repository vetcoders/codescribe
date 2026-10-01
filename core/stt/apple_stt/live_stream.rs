//! Long-lived Apple `stream` client — system-dictation shape.
//!
//! One subprocess, one `SFSpeechAudioBufferRecognitionRequest`, progressive
//! NDJSON events while PCM is still flowing:
//! - `partial` → open interim hypothesis
//! - `final`   → phrase seal (multi-utterance freezed+append fuel)
//! - closing `BridgeResponse` → summary after stdin EOF
//!
//! The Apple live session maps these onto
//! `EngineEvent::{Preview,UtteranceFinal}` before ledger admission. The `wav`
//! A/B alternative is an older Apple temp-WAV `transcribe_live` request
//! transport, not a VAD/scheduler route.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{
    BridgeRequest, BridgeResponse, ENV_ALLOW_DOWNLOAD, bridge_binary, bridge_global_lock, env_bool,
    resolved_locale,
};
use crate::pipeline::contracts::TranscriptSegment;

/// Progressive events from a live `stream` bridge session.
#[derive(Debug, Clone)]
pub enum LiveStreamEvent {
    Ready,
    Partial {
        text: String,
        segments: Vec<TranscriptSegment>,
    },
    /// Phrase-level seal (`isFinal` mid-stream). Text is utterance-local.
    PhraseFinal {
        text: String,
        segments: Vec<TranscriptSegment>,
    },
    End,
    Error {
        message: String,
    },
    /// Closing summary line (no `"event"` key). Always last.
    Summary {
        text: String,
        segments: Vec<TranscriptSegment>,
        ok: bool,
        error: Option<String>,
    },
}

/// Wire shape of a progress NDJSON line. Every field is optional because one
/// struct covers all `event` kinds — which key is populated depends on the kind.
#[derive(Debug, Deserialize)]
struct StreamEventLine {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    segments: Option<Vec<StreamSegmentLine>>,
    #[serde(default)]
    message: Option<String>,
    // Present on error events; summary lines use BridgeResponse path.
    #[serde(default)]
    error: Option<String>,
}

/// Wire shape of one timed segment inside a `final` event, before validation into
/// a [`TranscriptSegment`].
#[derive(Debug, Deserialize)]
struct StreamSegmentLine {
    text: String,
    start_ts: f32,
    end_ts: f32,
    /// `SFTranscriptionSegment.confidence` (0…1) on finals; 0.0 on partials
    /// is Apple's "no metric" sentinel. Consumed by the A6 confidence lane.
    #[serde(default)]
    confidence: Option<f32>,
}

/// Long-lived stream session: write PCM, poll progressive events, finish on drop/EOF.
pub struct LiveStreamSession {
    child: Child,
    stdin: Option<ChildStdin>,
    events_rx: Receiver<LiveStreamEvent>,
    reader: Option<JoinHandle<()>>,
    /// Held for the whole session so SFSpeech never races a concurrent bridge.
    _lock_guard: std::sync::MutexGuard<'static, ()>,
    sample_rate: u32,
    frames_written: u64,
}

impl LiveStreamSession {
    /// Spawn bridge `stream`, send request + PCM header, start stdout reader.
    pub fn open(language: Option<&str>, sample_rate: u32) -> Result<Self> {
        Self::open_cancellable(language, sample_rate, &AtomicBool::new(false))
    }

    /// The bridge thread may abandon an unopened epoch without waiting for
    /// another take to release the process-wide bridge mutex.
    pub(crate) fn open_cancellable(
        language: Option<&str>,
        sample_rate: u32,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        if sample_rate == 0 {
            bail!("live stream: sample_rate must be > 0");
        }
        let lock_guard = loop {
            if cancelled.load(Ordering::Acquire) {
                bail!("live stream: opening cancelled");
            }
            match bridge_global_lock().try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        };

        let locale = resolved_locale(language);
        let bridge_bin = bridge_binary();
        let mut child = Command::new(&bridge_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to spawn Apple STT bridge for live stream '{}'",
                    bridge_bin.display()
                )
            })?;

        let mut stdin = child
            .stdin
            .take()
            .context("Apple STT bridge stdin unavailable")?;
        // Every `write_pcm` below targets this pipe. Without this the first
        // write after the bridge dies raises SIGPIPE, which is fatal in the
        // Swift host and leaves no crash report — killing the bridge took the
        // whole app down on 2026-08-12. See `util::pipes`.
        crate::util::pipes::disable_sigpipe(&stdin);
        let stdout = child
            .stdout
            .take()
            .context("Apple STT bridge stdout unavailable")?;

        let contextual_strings: Option<Vec<String>> = None;
        let request = BridgeRequest {
            protocol_version: 1,
            command: "stream",
            locale: &locale,
            audio_path: None,
            contextual_strings: contextual_strings.as_deref(),
            allow_download: env_bool(ENV_ALLOW_DOWNLOAD, true),
            deadline_policy: None,
        };
        let req_payload = serde_json::to_vec(&request).context("serialize stream request")?;
        stdin
            .write_all(&req_payload)
            .context("write stream request")?;
        stdin.write_all(b"\n").context("write stream request EOL")?;
        let header = format!("{{\"rate\":{},\"channels\":1}}\n", sample_rate as f64);
        stdin
            .write_all(header.as_bytes())
            .context("write stream PCM header")?;
        stdin.flush().context("flush stream header")?;

        let (events_tx, events_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            read_stream_stdout(stdout, events_tx);
        });

        Ok(Self {
            child,
            stdin: Some(stdin),
            events_rx,
            reader: Some(reader),
            _lock_guard: lock_guard,
            sample_rate,
            frames_written: 0,
        })
    }

    /// The rate declared to the bridge at open time; PCM fed to
    /// [`LiveStreamSession::write_pcm`] must match it.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Append mono f32le frames into the long-lived SFSpeech request.
    pub fn write_pcm(&mut self, samples: &[f32]) -> Result<()> {
        let Some(stdin) = self.stdin.as_mut() else {
            bail!("live stream: stdin already closed");
        };
        if samples.is_empty() {
            return Ok(());
        }
        let mut pcm = Vec::with_capacity(samples.len() * 4);
        for &sample in samples {
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        stdin
            .write_all(&pcm)
            .context("write live stream PCM frames")?;
        self.frames_written += samples.len() as u64;
        Ok(())
    }

    /// Bound pipe writes on the bridge thread so cancellation cannot strand
    /// a mutex owner behind a child that has stopped reading stdin.
    #[cfg(unix)]
    pub(crate) fn write_pcm_cancellable(
        &mut self,
        samples: &[f32],
        cancelled: &AtomicBool,
    ) -> Result<()> {
        use std::os::fd::AsRawFd;

        const WRITE_BUDGET: Duration = Duration::from_millis(250);
        let stdin = self.stdin.as_mut().context("live stream: stdin closed")?;
        let fd = stdin.as_raw_fd();
        // SAFETY: stdin owns this valid descriptor; changing its status flags
        // neither transfers ownership nor invalidates it.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error()).context("read stream pipe flags");
        }
        // SAFETY: the same owned descriptor remains valid; O_NONBLOCK only
        // changes IO behaviour on this private child stdin pipe.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error()).context("set stream pipe nonblocking");
        }
        let pcm: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        let deadline = std::time::Instant::now() + WRITE_BUDGET;
        let mut written = 0;
        while written < pcm.len() {
            if cancelled.load(Ordering::Acquire) || std::time::Instant::now() >= deadline {
                bail!("live stream: pipe write cancelled or exceeded 250 ms");
            }
            match stdin.write(&pcm[written..]) {
                Ok(0) => bail!("live stream: pipe write returned zero"),
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(error).context("write live stream PCM frames"),
            }
        }
        self.frames_written += samples.len() as u64;
        Ok(())
    }

    #[cfg(not(unix))]
    pub(crate) fn write_pcm_cancellable(
        &mut self,
        _samples: &[f32],
        _cancelled: &AtomicBool,
    ) -> Result<()> {
        bail!("live stream: nonblocking pipe transport requires Unix");
    }

    /// Non-blocking drain of progressive events so far.
    pub fn poll_events(&mut self) -> Vec<LiveStreamEvent> {
        let mut out = Vec::new();
        loop {
            match self.events_rx.try_recv() {
                Ok(ev) => out.push(ev),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Close stdin (EOF), wait for reader + child, return remaining events incl. summary.
    pub fn finish(self) -> Result<Vec<LiveStreamEvent>> {
        self.finish_cancellable(&AtomicBool::new(false))
    }

    pub(crate) fn finish_cancellable(mut self, cancelled: &AtomicBool) -> Result<Vec<LiveStreamEvent>> {
        // EOF ends the stream session on the bridge side.
        drop(self.stdin.take());

        let fed_secs = self.frames_written as f64 / self.sample_rate.max(1) as f64;
        // Hard ceiling for a healthy-but-slow flush (mirror bridge settle grace
        // upper bound ~6s + process exit margin). A wedged bridge is cut far
        // earlier by the idle cutoff below, so this never pins a short clip.
        let wait = Duration::from_secs_f64((fed_secs + 30.0).clamp(45.0, 240.0));
        let deadline = std::time::Instant::now() + wait;
        // A live flush keeps emitting partial/final events; total silence past
        // the bridge's own settle grace means the child is wedged (alive but
        // mute). Without this, a 1-second clip waited the full 45s floor with
        // the overlay stuck in "finalising" and no way to cancel.
        /// Silence budget after the last event before a wedged bridge is killed.
        const IDLE_CUTOFF: Duration = Duration::from_secs(10);
        let mut last_event_at = std::time::Instant::now();

        let mut events = Vec::new();
        loop {
            if cancelled.load(Ordering::Acquire) {
                bail!("live stream: finish cancelled");
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self
                .events_rx
                .recv_timeout(remaining.min(Duration::from_millis(200)))
            {
                Ok(ev) => {
                    last_event_at = std::time::Instant::now();
                    let is_terminal = matches!(
                        ev,
                        LiveStreamEvent::Summary { .. } | LiveStreamEvent::Error { .. }
                    );
                    events.push(ev);
                    if is_terminal {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Keep waiting until hard deadline unless reader already exited.
                    if self.reader.as_ref().is_some_and(|h| h.is_finished()) {
                        // Drain any last bits.
                        events.extend(self.poll_events());
                        break;
                    }
                    if last_event_at.elapsed() >= IDLE_CUTOFF {
                        // Wedged bridge: give up on the summary, return the
                        // progressive events already sealed; the kill below
                        // reaps the mute child.
                        events.extend(self.poll_events());
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    events.extend(self.poll_events());
                    break;
                }
            }
        }

        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        // Reap child; ignore exit code if we already have a Summary.
        let _ = self.child.try_wait();
        let _ = self.child.kill();
        let _ = self.child.wait();

        Ok(events)
    }
}

impl Drop for LiveStreamSession {
    /// Best-effort teardown: close stdin, kill the bridge child, join the reader.
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

/// Reader-thread body: parse bridge stdout line by line and forward typed events.
///
/// Stops on a terminal event, an unparsable line ending the stream, or a closed
/// receiver — so a dropped session does not leave this thread pinned to a live
/// pipe. Lines that fail to parse are skipped rather than killing the session.
fn read_stream_stdout(stdout: impl std::io::Read, tx: Sender<LiveStreamEvent>) {
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(ev) = parse_stream_stdout_line(trimmed) {
            let terminal = matches!(
                ev,
                LiveStreamEvent::Summary { .. } | LiveStreamEvent::Error { .. }
            );
            if tx.send(ev).is_err() {
                break;
            }
            if terminal {
                break;
            }
        }
    }
}

/// Parse one NDJSON line from stream stdout into a typed event.
///
/// Progress lines carry `"event"`; the summary is a normal BridgeResponse
/// (`ok`/`status`, no `event`). Shared by the live session and unit tests.
pub(crate) fn parse_stream_stdout_line(line: &str) -> Option<LiveStreamEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Fast path: progress lines always include "event".
    if trimmed.contains("\"event\"") {
        let parsed: StreamEventLine = serde_json::from_str(trimmed).ok()?;
        let kind = parsed.event.as_deref()?.to_ascii_lowercase();
        return match kind.as_str() {
            "ready" => Some(LiveStreamEvent::Ready),
            "partial" => {
                let text = parsed.text.unwrap_or_default().trim().to_string();
                let segments = parsed
                    .segments
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|s| {
                        let text = s.text.trim().to_string();
                        if text.is_empty()
                            || !s.start_ts.is_finite()
                            || !s.end_ts.is_finite()
                            || s.end_ts < s.start_ts
                        {
                            return None;
                        }
                        Some(TranscriptSegment {
                            text,
                            start_ts: s.start_ts,
                            end_ts: s.end_ts,
                            // Partials carry `confidence: 0.0` per Apple
                            // semantics — that means "no metric", never "low"
                            // (A6, d4). Only `final` rows below may carry one.
                            confidence: None,
                        })
                    })
                    .collect();
                Some(LiveStreamEvent::Partial { text, segments })
            }
            "final" => {
                let text = parsed.text.unwrap_or_default().trim().to_string();
                let segments = parsed
                    .segments
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|s| {
                        let t = s.text.trim().to_string();
                        if t.is_empty()
                            || !s.start_ts.is_finite()
                            || !s.end_ts.is_finite()
                            || s.end_ts < s.start_ts
                        {
                            return None;
                        }
                        // A 0.0 confidence is Apple's "no metric" sentinel on
                        // frozen partials emitted as finals; it is not a
                        // measurement (A6, d4: never invent confidence).
                        let confidence = s
                            .confidence
                            .filter(|value| value.is_finite() && *value > 0.0)
                            .map(|value| {
                                crate::pipeline::word_confidence::WordConfidence::new(
                                    crate::pipeline::word_confidence::WordConfidenceSource::AppleSegmentConfidence,
                                    value,
                                    1,
                                )
                            });
                        Some(TranscriptSegment {
                            text: t,
                            start_ts: s.start_ts,
                            end_ts: s.end_ts,
                            confidence,
                        })
                    })
                    .collect();
                Some(LiveStreamEvent::PhraseFinal { text, segments })
            }
            "end" => Some(LiveStreamEvent::End),
            "error" => Some(LiveStreamEvent::Error {
                message: parsed
                    .message
                    .or(parsed.error)
                    .unwrap_or_else(|| "stream error".into()),
            }),
            _ => None,
        };
    }

    // Closing BridgeResponse (summary).
    if let Ok(resp) = serde_json::from_str::<BridgeResponse>(trimmed) {
        let ok = resp.is_ok();
        let segments = resp
            .segments
            .into_iter()
            .filter_map(super::bridge_segment_to_transcript_segment)
            .collect();
        return Some(LiveStreamEvent::Summary {
            text: resp.text.trim().to_string(),
            segments,
            ok,
            error: resp.error,
        });
    }
    None
}

/// Compatibility accessor for the Apple bridge transport token.
///
/// `stream` selects progressive Apple AudioBuffer delivery; `wav` and
/// `transcribe_live` name the older Apple temp-WAV request transport. A fresh
/// Loctree 600-file structural census on 2026-08-25 found this definition and
/// its re-export but no caller. That is a structural observation, not runtime
/// proof, and this helper does not select or restore a VAD/scheduler pipeline.
pub fn progressive_live_enabled() -> bool {
    let mode = std::env::var("CODESCRIBE_APPLE_STT_LIVE_MODE").unwrap_or_else(|_| "stream".into());
    !mode.eq_ignore_ascii_case("wav") && !mode.eq_ignore_ascii_case("transcribe_live")
}

/// Bridge stdout line parsers for progressive multi-seal events.
#[cfg(test)]
mod tests {
    use super::*;

    /// Partial, phrase-final, and summary JSON lines map to the typed event enum.
    #[test]
    fn parse_partial_and_final_and_summary() {
        let partial = parse_stream_stdout_line(
            r#"{"event":"partial","text":"cześć","segments":[{"text":"cześć","start_ts":0.0,"end_ts":0.4,"confidence":0.9}]}"#,
        )
        .expect("partial");
        match partial {
            LiveStreamEvent::Partial { text, segments } => {
                assert_eq!(text, "cześć");
                assert_eq!(segments.len(), 1);
                assert_eq!(segments[0].text, "cześć");
                assert_eq!(segments[0].start_ts, 0.0);
                assert_eq!(segments[0].end_ts, 0.4);
            }
            other => panic!("expected Partial, got {other:?}"),
        }

        let final_line = parse_stream_stdout_line(
            r#"{"event":"final","text":"cześć świecie","segments":[{"text":"cześć","start_ts":0.0,"end_ts":0.4,"confidence":0.9}]}"#,
        )
        .expect("final");
        match final_line {
            LiveStreamEvent::PhraseFinal { text, segments } => {
                assert_eq!(text, "cześć świecie");
                assert_eq!(segments.len(), 1);
            }
            other => panic!("expected PhraseFinal, got {other:?}"),
        }

        let summary = parse_stream_stdout_line(
            r#"{"ok":true,"status":"ok","text":"cześć świecie","segments":[],"backend":"sf_speech_recognizer"}"#,
        )
        .expect("summary");
        match summary {
            LiveStreamEvent::Summary { text, ok, .. } => {
                assert!(ok);
                assert_eq!(text, "cześć świecie");
            }
            other => panic!("expected Summary, got {other:?}"),
        }
    }

    /// A6 (e): Apple confidence survives only real finals. Partials and the
    /// 0.0 "no metric" sentinel become `None` — never low-confidence words.
    #[test]
    fn apple_confidence_only_counts_measured_finals() {
        let partial = parse_stream_stdout_line(
            r#"{"event":"partial","text":"cześć","segments":[{"text":"cześć","start_ts":0.0,"end_ts":0.4,"confidence":0.9}]}"#,
        )
        .expect("partial");
        match partial {
            LiveStreamEvent::Partial { segments, .. } => {
                assert_eq!(segments[0].confidence, None);
            }
            other => panic!("expected Partial, got {other:?}"),
        }

        let frozen_final = parse_stream_stdout_line(
            r#"{"event":"final","text":"cześć","segments":[{"text":"cześć","start_ts":0.0,"end_ts":0.4,"confidence":0.0}]}"#,
        )
        .expect("frozen final");
        match frozen_final {
            LiveStreamEvent::PhraseFinal { segments, .. } => {
                assert_eq!(segments[0].confidence, None);
            }
            other => panic!("expected PhraseFinal, got {other:?}"),
        }

        let attributeless = parse_stream_stdout_line(
            r#"{"event":"final","text":"cześć","segments":[{"text":"cześć","start_ts":0.0,"end_ts":0.4}]}"#,
        )
        .expect("attributeless final");
        match attributeless {
            LiveStreamEvent::PhraseFinal { segments, .. } => {
                assert_eq!(segments[0].confidence, None);
            }
            other => panic!("expected PhraseFinal, got {other:?}"),
        }

        let measured = parse_stream_stdout_line(
            r#"{"event":"final","text":"cześć","segments":[{"text":"cześć","start_ts":0.0,"end_ts":0.4,"confidence":0.42}]}"#,
        )
        .expect("measured final");
        match measured {
            LiveStreamEvent::PhraseFinal { segments, .. } => {
                let confidence = segments[0].confidence.expect("final carries confidence");
                assert_eq!(
                    confidence.source,
                    crate::pipeline::word_confidence::WordConfidenceSource::AppleSegmentConfidence
                );
                assert!((confidence.value() - 0.42).abs() < 0.001);
            }
            other => panic!("expected PhraseFinal, got {other:?}"),
        }
    }

    /// Each `final` event is its own sealed phrase; summary is not a full replace.
    #[test]
    fn multi_phrase_finals_are_independent_seals() {
        let lines = [
            r#"{"event":"ready"}"#,
            r#"{"event":"partial","text":"pierwsze"}"#,
            r#"{"event":"final","text":"pierwsze zdanie"}"#,
            r#"{"event":"partial","text":"drugie"}"#,
            r#"{"event":"final","text":"drugie zdanie"}"#,
            r#"{"event":"end"}"#,
            r#"{"ok":true,"status":"ok","text":"pierwsze zdanie drugie zdanie","segments":[]}"#,
        ];
        let events: Vec<_> = lines
            .iter()
            .filter_map(|l| parse_stream_stdout_line(l))
            .collect();
        let seals: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                LiveStreamEvent::PhraseFinal { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(seals, ["pierwsze zdanie", "drugie zdanie"]);
    }
}
