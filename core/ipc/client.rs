use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

use super::{IpcCommand, IpcResponse, socket_path};

const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[derive(Debug, Clone)]
pub struct IpcClient {
    socket_path: PathBuf,
    response_timeout: Duration,
}

impl Default for IpcClient {
    fn default() -> Self {
        Self::new(socket_path())
    }
}

impl IpcClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            response_timeout: DEFAULT_RESPONSE_TIMEOUT,
        }
    }

    pub fn with_response_timeout(mut self, response_timeout: Duration) -> Self {
        self.response_timeout = response_timeout;
        self
    }

    pub async fn transcribe_file(&self, path: &Path) -> Result<String> {
        let response = self
            .send_command(IpcCommand::TranscribeFile {
                path: path.to_string_lossy().into_owned(),
            })
            .await?;

        match response {
            IpcResponse::Message(text) => Ok(text),
            IpcResponse::Error(message) => bail!("Codescribe IPC transcription failed: {message}"),
            other => bail!("Unexpected Codescribe IPC response: {other:?}"),
        }
    }

    async fn send_command(&self, command: IpcCommand) -> Result<IpcResponse> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| {
                format!(
                    "Codescribe IPC unavailable at {}",
                    self.socket_path.display()
                )
            })?;

        let payload =
            serde_json::to_string(&command).context("Failed to encode Codescribe IPC command")?;
        stream
            .write_all(payload.as_bytes())
            .await
            .context("Failed to write Codescribe IPC command")?;
        stream
            .write_all(b"\n")
            .await
            .context("Failed to terminate Codescribe IPC command")?;
        stream
            .flush()
            .await
            .context("Failed to flush Codescribe IPC command")?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let bytes_read = timeout(self.response_timeout, reader.read_line(&mut line))
            .await
            .map_err(|_| anyhow!("Codescribe IPC response timed out"))?
            .context("Failed to read Codescribe IPC response")?;

        if bytes_read == 0 {
            bail!("Codescribe IPC closed before sending a response");
        }

        serde_json::from_str::<IpcResponse>(&line).with_context(|| {
            format!(
                "Malformed Codescribe IPC response from {}",
                self.socket_path.display()
            )
        })
    }
}

pub async fn transcribe_file(path: &Path) -> Result<String> {
    IpcClient::default().transcribe_file(path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_socket_returns_unavailable_error() {
        let temp = tempfile::tempdir().expect("create temp dir for ipc client test");
        let socket = temp.path().join("missing-codescribe.sock");
        let audio = temp.path().join("audio.wav");
        let client = IpcClient::new(socket);

        let err = client
            .transcribe_file(&audio)
            .await
            .expect_err("missing socket should fail without local STT fallback");
        let message = err.to_string();

        assert!(
            message.contains("Codescribe IPC unavailable"),
            "expected unavailable error, got: {message}"
        );
    }
}
