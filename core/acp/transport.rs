//! Duplex NDJSON process transport. One task owns all three child pipes.

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::protocol::{diagnostic_prefix, notification, parse_frame, redact_diagnostic};
use super::{AcpCommand, AcpError};

/// A frame is bounded before parsing; a missing newline cannot grow forever.
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const STDERR_LINES: usize = 64;
const STDERR_LINE_BYTES: usize = 4096;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_TIMEOUT: Duration = Duration::from_millis(100);

enum Outbound {
    Write(Vec<u8>, oneshot::Sender<Result<(), AcpError>>),
    BindSession(String),
    Shutdown(Option<String>, Option<oneshot::Sender<()>>),
}

/// Process ownership stays with this handle, including when its caller drops
/// a pending turn future. Dropping it requests cancel and then kills/reaps.
pub struct AcpTransport {
    outgoing: mpsc::UnboundedSender<Outbound>,
    incoming: mpsc::Receiver<Result<Value, AcpError>>,
    process_id: Option<u32>,
    actor: Option<JoinHandle<()>>,
}

impl AcpTransport {
    /// Spawn a configured executable directly, never through an implicit shell.
    pub async fn spawn(command: &AcpCommand) -> Result<Self, AcpError> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .current_dir(&command.cwd)
            .envs(&command.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(AcpError::Io)?;
        let process_id = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AcpError::Protocol("ACP stdin unavailable".into()))?;
        crate::util::pipes::disable_sigpipe(&stdin);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AcpError::Protocol("ACP stdout unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AcpError::Protocol("ACP stderr unavailable".into()))?;
        let diagnostics = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_LINES)));
        let stderr_task = tokio::spawn(drain_stderr(stderr, diagnostics.clone()));
        let (outgoing, commands) = mpsc::unbounded_channel();
        let (frames, incoming) = mpsc::channel(64);
        let actor = tokio::spawn(run_process(
            child,
            stdin,
            BufReader::new(stdout),
            commands,
            frames,
            diagnostics,
            stderr_task,
        ));
        Ok(Self {
            outgoing,
            incoming,
            process_id,
            actor: Some(actor),
        })
    }

    /// The spawned PID, useful for an exact teardown receipt.
    pub fn id(&self) -> Option<u32> {
        self.process_id
    }

    /// Bind teardown to the actual session returned by session/new or load.
    pub fn set_session_id(&mut self, session_id: String) {
        let _ = self.outgoing.send(Outbound::BindSession(session_id));
    }

    /// Write exactly one JSON-RPC frame and its newline; await actual delivery
    /// to the pipe rather than treating queue acceptance as delivery.
    pub async fn send(&self, frame: Value) -> Result<(), AcpError> {
        let mut bytes =
            serde_json::to_vec(&frame).map_err(|error| AcpError::Protocol(error.to_string()))?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(AcpError::Protocol(
                "ACP outgoing frame exceeds byte limit".into(),
            ));
        }
        parse_frame(&bytes)?;
        bytes.push(b'\n');
        let (tx, rx) = oneshot::channel();
        self.outgoing
            .send(Outbound::Write(bytes, tx))
            .map_err(|_| AcpError::ProcessExited("ACP process transport closed".into()))?;
        rx.await
            .map_err(|_| AcpError::ProcessExited("ACP process transport closed".into()))?
    }

    /// Cancellation-safe: cancelling this receive cannot lose a partial line.
    pub async fn recv(&mut self) -> Result<Value, AcpError> {
        self.incoming
            .recv()
            .await
            .unwrap_or_else(|| Err(AcpError::ProcessExited("ACP stdout closed".into())))
    }

    /// Send session/cancel before closing stdin, then kill and reap the child.
    pub async fn shutdown(&mut self, session_id: Option<&str>) {
        // Release a process actor suspended on a full inbound queue, too.
        self.incoming.close();
        let (tx, rx) = oneshot::channel();
        let _ = self
            .outgoing
            .send(Outbound::Shutdown(session_id.map(str::to_string), Some(tx)));
        let _ = rx.await;
        if let Some(actor) = self.actor.take() {
            let _ = actor.await;
        }
    }
}

impl Drop for AcpTransport {
    fn drop(&mut self) {
        // No spawn/block_on here: dropping outside a runtime must not panic.
        // The existing actor owns kill_on_drop and the session-bound cancel.
        let _ = self.outgoing.send(Outbound::Shutdown(None, None));
    }
}

async fn write_frame(
    stdin: &mut ChildStdin,
    bytes: &[u8],
    budget: Duration,
) -> Result<(), AcpError> {
    tokio::time::timeout(budget, async {
        stdin.write_all(bytes).await?;
        stdin.flush().await
    })
    .await
    .map_err(|_| AcpError::Protocol("ACP stdin write timed out".into()))?
    .map_err(AcpError::Io)
}

async fn run_process(
    mut child: Child,
    mut stdin: ChildStdin,
    mut stdout: BufReader<tokio::process::ChildStdout>,
    mut commands: mpsc::UnboundedReceiver<Outbound>,
    frames: mpsc::Sender<Result<Value, AcpError>>,
    diagnostics: Arc<Mutex<VecDeque<String>>>,
    mut stderr_task: JoinHandle<()>,
) {
    let mut session_id = None;
    let mut line = Vec::new();
    let mut completion = None;
    let mut terminal_error = None;
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(Outbound::Write(bytes, ack)) => {
                    let result = write_frame(&mut stdin, &bytes, WRITE_TIMEOUT).await;
                    let failed = result.is_err();
                    let _ = ack.send(result);
                    if failed { break; }
                }
                Some(Outbound::BindSession(id)) => session_id = Some(id),
                Some(Outbound::Shutdown(id, ack)) => {
                    session_id = id.or(session_id);
                    completion = ack;
                    break;
                }
                None => break,
            },
            result = read_frame(&mut stdout, &mut line) => match result {
                Ok(Some(frame)) => {
                    if frames.send(Ok(frame)).await.is_err() { break; }
                }
                Ok(None) => {
                    terminal_error = Some(AcpError::ProcessExited("ACP stdout reached EOF".into()));
                    break;
                }
                Err(error) => {
                    terminal_error = Some(error);
                    break;
                }
            },
        }
    }
    if let Some(id) = session_id
        && let Ok(mut bytes) = serde_json::to_vec(&notification(
            "session/cancel",
            serde_json::json!({"sessionId":id}),
        ))
    {
        bytes.push(b'\n');
        let _ = write_frame(&mut stdin, &bytes, CANCEL_TIMEOUT).await;
    }
    drop(stdin);
    // A normal cancel gets a short grace period to observe the notification.
    // Malformed output and EOF are failures and kill immediately instead.
    if terminal_error.is_none() {
        let _ = tokio::time::timeout(CANCEL_TIMEOUT, child.wait()).await;
    }
    let status = match child.try_wait() {
        Ok(Some(status)) => Some(status),
        _ => {
            let _ = child.start_kill();
            child.wait().await.ok()
        }
    };
    if tokio::time::timeout(CANCEL_TIMEOUT, &mut stderr_task)
        .await
        .is_err()
    {
        stderr_task.abort();
        let _ = stderr_task.await;
    }
    if let Some(error) = terminal_error {
        let error = match error {
            AcpError::ProcessExited(reason) => {
                let stderr = diagnostics
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                AcpError::ProcessExited(format!(
                    "{reason}; status={status:?}; stderr (last 64 lines): {stderr}"
                ))
            }
            other => other,
        };
        let _ = frames.send(Err(error)).await;
    }
    if let Some(ack) = completion {
        let _ = ack.send(());
    }
}

async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut Vec<u8>,
) -> Result<Option<Value>, AcpError> {
    loop {
        let buffer = reader.fill_buf().await.map_err(AcpError::Io)?;
        if buffer.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            // NDJSON requires a newline even for the last frame.
            return Err(AcpError::MalformedFrame(diagnostic_prefix(
                &String::from_utf8_lossy(line),
            )));
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |at| at + 1);
        let content = newline.unwrap_or(consumed);
        if line.len() + content > MAX_FRAME_BYTES {
            return Err(AcpError::MalformedFrame(format!(
                "ACP frame exceeds byte limit: {}",
                diagnostic_prefix(&String::from_utf8_lossy(line))
            )));
        }
        line.extend_from_slice(&buffer[..content]);
        reader.consume(consumed);
        if newline.is_some() {
            let parsed = parse_frame(line);
            line.clear();
            return parsed.map(Some);
        }
    }
}

async fn drain_stderr(
    mut stderr: tokio::process::ChildStderr,
    diagnostics: Arc<Mutex<VecDeque<String>>>,
) {
    let mut bytes = [0_u8; 1024];
    let mut line = Vec::new();
    let mut truncated = false;
    loop {
        let count = match stderr.read(&mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        for byte in &bytes[..count] {
            if *byte == b'\n' {
                push_stderr(&diagnostics, &line, truncated);
                line.clear();
                truncated = false;
            } else if line.len() < STDERR_LINE_BYTES {
                line.push(*byte);
            } else {
                truncated = true;
            }
        }
    }
    if !line.is_empty() || truncated {
        push_stderr(&diagnostics, &line, truncated);
    }
}

fn push_stderr(diagnostics: &Mutex<VecDeque<String>>, line: &[u8], truncated: bool) {
    let text = redact_diagnostic(&String::from_utf8_lossy(line));
    let text = if truncated {
        format!("{text} [truncated]")
    } else {
        text
    };
    let mut ring = diagnostics
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if ring.len() == STDERR_LINES {
        ring.pop_front();
    }
    ring.push_back(text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn shell(script: &str) -> AcpCommand {
        AcpCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn acp_transport_sends_only_ndjson_and_receives_duplex_frames() {
        let mut transport = AcpTransport::spawn(&shell(
            "while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
        ))
        .await
        .unwrap();
        let frame = super::super::protocol::request(
            7,
            "session/prompt",
            serde_json::json!({"sessionId":"s", "prompt":[{"type":"text","text":"line one\nline two"}]}),
        );
        transport.send(frame.clone()).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), transport.recv())
                .await
                .unwrap()
                .unwrap(),
            frame
        );
        transport.shutdown(None).await;
    }

    #[tokio::test]
    async fn acp_malformed_banner_is_redacted_and_process_is_reaped() {
        let mut transport = AcpTransport::spawn(&shell(
            "printf 'TOKEN=private-value garbage\\n'; exec /bin/sleep 30",
        ))
        .await
        .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(3), transport.recv())
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, AcpError::MalformedFrame(_)));
        assert!(!error.to_string().contains("private-value"));
        transport.shutdown(None).await;
    }

    #[tokio::test]
    async fn acp_eof_carries_only_last_64_redacted_stderr_lines() {
        let script = "i=0; while [ $i -lt 70 ]; do printf 'line-%s TOKEN=private-value\\n' \"$i\" >&2; i=$((i+1)); done";
        let mut transport = AcpTransport::spawn(&shell(script)).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(3), transport.recv())
            .await
            .unwrap()
            .unwrap_err();
        let diagnostic = error.to_string();
        assert!(matches!(error, AcpError::ProcessExited(_)));
        assert!(!diagnostic.contains("line-0 "));
        assert!(diagnostic.contains("line-69 "));
        assert!(!diagnostic.contains("private-value"));
        transport.shutdown(None).await;
    }

    #[tokio::test]
    async fn acp_cancel_is_written_before_process_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let receipt = directory.path().join("cancel.json");
        let script = format!(
            "IFS= read -r line; printf '%s' \"$line\" > '{}'",
            receipt.display()
        );
        let mut transport = AcpTransport::spawn(&shell(&script)).await.unwrap();
        transport.set_session_id("session-exact".into());
        transport.shutdown(Some("session-exact")).await;
        let frame: Value = serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
        assert_eq!(frame["method"], "session/cancel");
        assert_eq!(frame["params"]["sessionId"], "session-exact");
    }
}
