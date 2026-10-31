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
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{
    BRIDGE_TEARDOWN_BUDGET, BridgeRequest, BridgeResponse, BridgeTeardownOutcome,
    ENV_ALLOW_DOWNLOAD, bridge_binary, bridge_global_lock, child_has_exited_without_reap,
    configure_bridge_process_group, env_bool, kill_and_reap_bridge_group_until, resolved_locale,
    teardown_bridge_reader,
};
use crate::pipeline::contracts::TranscriptSegment;

/// Wall-clock budget for one stream handshake or `write_pcm` call into the
/// bridge stdin pipe when the child stops draining it.
const LIVE_STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

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
    teardown_outcome: Option<BridgeTeardownOutcome>,
    /// Held for the whole session so SFSpeech never races a concurrent bridge.
    _lock_guard: std::sync::MutexGuard<'static, ()>,
    sample_rate: u32,
    frames_written: u64,
}

impl LiveStreamSession {
    /// Spawn bridge `stream`, send request + PCM header, start stdout reader.
    pub fn open(language: Option<&str>, sample_rate: u32) -> Result<Self> {
        if sample_rate == 0 {
            bail!("live stream: sample_rate must be > 0");
        }
        let lock_guard = bridge_global_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let locale = resolved_locale(language);
        let bridge_bin = bridge_binary();
        let mut command = Command::new(&bridge_bin);
        configure_bridge_process_group(&mut command);
        let mut child = command
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

        let handshake = (|| -> Result<(ChildStdin, std::process::ChildStdout)> {
            let mut stdin = child
                .stdin
                .take()
                .context("Apple STT bridge stdin unavailable")?;
            // Every `write_pcm` below targets this pipe. Without this the first
            // write after the bridge dies raises SIGPIPE, which is fatal in the
            // Swift host and leaves no crash report — killing the bridge took the
            // whole app down on 2026-08-12. See `util::pipes`.
            crate::util::pipes::disable_sigpipe(&stdin);
            set_nonblocking(&stdin).context("set Apple STT bridge stdin non-blocking")?;
            let stdout = child
                .stdout
                .take()
                .context("Apple STT bridge stdout unavailable")?;

            let vocabulary =
                crate::stt::recognizer_vocabulary::RecognizerVocabulary::load_read_only();
            let request = BridgeRequest {
                protocol_version: 1,
                command: "stream",
                locale: &locale,
                audio_path: None,
                contextual_strings: vocabulary.apple_contextual_strings(),
                allow_download: env_bool(ENV_ALLOW_DOWNLOAD, true),
                deadline_policy: None,
                backend: None,
            };
            let req_payload = serde_json::to_vec(&request).context("serialize stream request")?;
            let handshake_deadline = std::time::Instant::now() + LIVE_STREAM_WRITE_TIMEOUT;
            write_all_until(&mut stdin, &req_payload, handshake_deadline)
                .context("write stream request")?;
            write_all_until(&mut stdin, b"\n", handshake_deadline)
                .context("write stream request EOL")?;
            let header = format!("{{\"rate\":{},\"channels\":1}}\n", sample_rate as f64);
            write_all_until(&mut stdin, header.as_bytes(), handshake_deadline)
                .context("write stream PCM header")?;
            Ok((stdin, stdout))
        })();
        let (stdin, stdout) = match handshake {
            Ok(handles) => handles,
            Err(error) => {
                let reaped = kill_and_reap_bridge_group_until(
                    &mut child,
                    false,
                    std::time::Instant::now() + BRIDGE_TEARDOWN_BUDGET,
                );
                if !reaped {
                    tracing::warn!(
                        pid = child.id(),
                        "Apple STT live stream open teardown did not reap child within budget"
                    );
                }
                return Err(error);
            }
        };

        let (events_tx, events_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            read_stream_stdout(stdout, events_tx);
        });

        Ok(Self {
            child,
            stdin: Some(stdin),
            events_rx,
            reader: Some(reader),
            teardown_outcome: None,
            _lock_guard: lock_guard,
            sample_rate,
            frames_written: 0,
        })
    }

    /// Idempotent bounded teardown of the bridge process group and stdout reader.
    /// Executes at most once per session so `child.id()` is never signalled after
    /// being reaped.
    fn teardown_bridge(&mut self) -> BridgeTeardownOutcome {
        drop(self.stdin.take());
        if let Some(outcome) = self.teardown_outcome {
            return outcome;
        }
        let deadline = std::time::Instant::now() + BRIDGE_TEARDOWN_BUDGET;
        let outcome = teardown_bridge_reader(&mut self.child, false, &mut self.reader, deadline);
        if !outcome.is_complete() {
            tracing::warn!(
                pid = self.child.id(),
                child_reaped = outcome.child_reaped,
                readers_joined = outcome.readers_joined,
                "Apple STT live stream bridge teardown did not complete within budget"
            );
        }
        self.teardown_outcome = Some(outcome);
        outcome
    }

    /// The rate declared to the bridge at open time; PCM fed to
    /// [`LiveStreamSession::write_pcm`] must match it.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Append mono f32le frames into the long-lived SFSpeech request.
    pub fn write_pcm(&mut self, samples: &[f32]) -> Result<()> {
        if samples.is_empty() {
            if self.stdin.is_none() {
                bail!("live stream: stdin already closed");
            }
            return Ok(());
        }
        let mut pcm = Vec::with_capacity(samples.len() * 4);
        for &sample in samples {
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        let deadline = std::time::Instant::now() + LIVE_STREAM_WRITE_TIMEOUT;
        let write_result = {
            let Some(stdin) = self.stdin.as_mut() else {
                bail!("live stream: stdin already closed");
            };
            write_all_until(stdin, &pcm, deadline).context("write live stream PCM frames")
        };
        if let Err(error) = write_result {
            let outcome = self.teardown_bridge();
            if !outcome.is_complete() {
                return Err(error.context(format!(
                    "Apple STT live stream bridge teardown incomplete after write failure (child_reaped={}, readers_joined={})",
                    outcome.child_reaped, outcome.readers_joined
                )));
            }
            return Err(error);
        }
        self.frames_written += samples.len() as u64;
        Ok(())
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
    pub fn finish(mut self) -> Result<Vec<LiveStreamEvent>> {
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
                    // Keep waiting until hard deadline unless reader already exited
                    // or the group leader has terminated while a descendant holds stdout open.
                    if self.reader.as_ref().is_some_and(|h| h.is_finished())
                        || child_has_exited_without_reap(&self.child).unwrap_or(false)
                    {
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

        // A blocked stdout read ends only when the entire bridge process group
        // closes its pipe. Terminate the group and drain the reader under one
        // shared deadline so a wedged bridge or grandchild cannot hang finish.
        let outcome = self.teardown_bridge();
        events.extend(self.poll_events());
        if !outcome.is_complete() {
            bail!(
                "Apple STT live stream bridge teardown incomplete after {}ms (child_reaped={}, readers_joined={})",
                BRIDGE_TEARDOWN_BUDGET.as_millis(),
                outcome.child_reaped,
                outcome.readers_joined
            );
        }

        Ok(events)
    }
}

impl Drop for LiveStreamSession {
    /// Bounded teardown: close stdin, kill the bridge process group, join the reader.
    fn drop(&mut self) {
        let _ = self.teardown_bridge();
    }
}

/// Put a child stdin pipe into non-blocking mode so writes surface
/// `ErrorKind::WouldBlock` instead of hanging the capture/session thread inside
/// the kernel when the bridge child stops draining stdin.
#[cfg(unix)]
fn set_nonblocking<F: std::os::fd::AsRawFd>(pipe: &F) -> std::io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: `fd` belongs to `pipe` for the duration of this call; `F_GETFL`
    // only reads descriptor status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `F_SETFL` sets `O_NONBLOCK` on a valid pipe descriptor owned by caller.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_nonblocking<F>(_pipe: &F) -> std::io::Result<()> {
    Ok(())
}

/// Wait until `pipe` becomes writable (or closed/errored by peer) before `deadline`.
#[cfg(unix)]
fn wait_stdin_writable_until<F: std::os::fd::AsRawFd>(
    pipe: &F,
    deadline: std::time::Instant,
) -> std::io::Result<()> {
    let fd = pipe.as_raw_fd();
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Apple STT bridge stdin remained full past write deadline",
            ));
        }
        let remaining = deadline.saturating_duration_since(now);
        let timeout_ms = remaining.as_millis().clamp(1, 50) as libc::c_int;
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: `pollfd` is a valid stack-allocated descriptor entry for `fd`.
        let rc = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if rc > 0 {
            if (pollfd.revents & libc::POLLNVAL) != 0 {
                return Err(std::io::Error::from_raw_os_error(libc::EBADF));
            }
            if (pollfd.revents & (libc::POLLOUT | libc::POLLHUP | libc::POLLERR)) != 0 {
                return Ok(());
            }
        } else if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
    }
}

#[cfg(not(unix))]
fn wait_stdin_writable_until<F>(_pipe: &F, deadline: std::time::Instant) -> std::io::Result<()> {
    if std::time::Instant::now() >= deadline {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Apple STT bridge stdin write timed out",
        ));
    }
    std::thread::sleep(Duration::from_millis(5));
    Ok(())
}

/// Write `bytes` completely in exact byte order before `deadline` without
/// resetting the deadline on partial progress.
fn write_all_until(
    stdin: &mut ChildStdin,
    bytes: &[u8],
    deadline: std::time::Instant,
) -> Result<()> {
    let mut offset = 0usize;
    let mut made_attempt = false;
    while offset < bytes.len() {
        if made_attempt && std::time::Instant::now() >= deadline {
            return Err(anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Apple STT bridge stdin write timed out after {offset}/{} bytes",
                    bytes.len()
                ),
            )));
        }
        made_attempt = true;
        match stdin.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(anyhow::Error::new(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    format!(
                        "Apple STT bridge stdin closed after {offset}/{} bytes",
                        bytes.len()
                    ),
                )));
            }
            Ok(written) => {
                offset += written;
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                wait_stdin_writable_until(stdin, deadline).with_context(|| {
                    format!(
                        "Apple STT bridge stdin write timed out after {offset}/{} bytes",
                        bytes.len()
                    )
                })?;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "write Apple STT bridge stdin ({offset}/{} bytes written)",
                        bytes.len()
                    )
                });
            }
        }
    }
    Ok(())
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

    #[test]
    fn finish_reaps_a_silent_bridge_before_joining_stdout() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn synthetic bridge");
        let stdout = child.stdout.take().expect("stdout");
        let stdin = child.stdin.take();
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || read_stream_stdout(stdout, tx));
        // Bound the regression itself: old finish blocks until this watchdog
        // terminates this synthetic process. It never targets an app process.
        let pid = child.id();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let watchdog = thread::spawn(move || {
            if cancel_rx.recv_timeout(Duration::from_secs(13)).is_err() {
                let _ = Command::new("/bin/kill").arg(pid.to_string()).status();
            }
        });
        static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let session = LiveStreamSession {
            child,
            stdin,
            events_rx: rx,
            reader: Some(reader),
            teardown_outcome: None,
            _lock_guard: TEST_LOCK.lock().expect("test lock"),
            sample_rate: 16_000,
            frames_written: 0,
        };
        let started = std::time::Instant::now();
        let events = session.finish().expect("finish");
        let elapsed = started.elapsed();
        let _ = cancel_tx.send(());
        watchdog.join().expect("watchdog");
        assert!(events.is_empty());
        assert!(elapsed < Duration::from_secs(12), "finish took {elapsed:?}");
    }

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
