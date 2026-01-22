//! Voice Chat WebSocket client for real-time audio streaming.
//!
//! This module provides a WebSocket client for the voice chat backend,
//! enabling streaming audio transcription and LLM responses.

// Allow unused API methods - they're part of the public interface for future use

use anyhow::{Context, Result};
use base64::prelude::*;
use crossbeam_channel::Sender;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Default backend URL if CODESCRIBE_BACKEND_URL is not set.
const DEFAULT_BACKEND_URL: &str = "http://127.0.0.1:8237";

/// Events received from the voice chat server.
#[derive(Debug, Clone)]
pub enum VoiceChatEvent {
    /// Connection established successfully.
    Connected,
    /// Partial transcript (speech-to-text in progress).
    Transcript(String),
    /// Complete sentence recognized.
    SentenceComplete(String),
    /// Streaming LLM token.
    LlmDelta(String),
    /// LLM response complete.
    LlmDone { text: String, response_id: String },
    /// Error from the server.
    Error(String),
    /// Connection closed.
    Disconnected,
}

/// Messages sent to the server.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    /// Audio chunk with base64-encoded data.
    Chunk {
        audio_base64: String,
        sample_rate: u32,
        last: bool,
    },
    /// Flush the transcription buffer.
    Flush,
    /// End the audio stream.
    End,
    /// Set the language for transcription.
    Set { language: String },
    /// Reset the session.
    Reset,
}

/// Messages received from the server.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    /// Initial handshake.
    Hello { protocol: String },
    /// Acknowledgment of received audio.
    Ack { received_bytes: u64 },
    /// Partial transcript.
    Transcript { text: String },
    /// Complete sentence.
    #[serde(rename = "sentence.complete")]
    SentenceComplete { text: String },
    /// Streaming LLM token.
    #[serde(rename = "llm.delta")]
    LlmDelta { delta: String },
    /// LLM response complete.
    #[serde(rename = "llm.done")]
    LlmDone { text: String, response_id: String },
    /// Error from server.
    Error { message: String },
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

/// WebSocket client for voice chat.
pub struct VoiceChatClient {
    sink: Arc<Mutex<WsSink>>,
    /// Handle to the receiver task - kept alive to prevent task cancellation.
    _receiver_handle: tokio::task::JoinHandle<()>,
}

impl VoiceChatClient {
    /// Connect to the voice chat WebSocket endpoint.
    ///
    /// # Arguments
    /// * `event_sender` - Channel sender for delivering events to the UI thread.
    ///
    /// # Returns
    /// A connected `VoiceChatClient` or an error.
    pub async fn connect(event_sender: Sender<VoiceChatEvent>) -> Result<Self> {
        let backend_url = std::env::var("CODESCRIBE_BACKEND_URL")
            .unwrap_or_else(|_| DEFAULT_BACKEND_URL.to_string());

        // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
        // Convert http(s):// to ws(s):// (ws:// for localhost dev, wss:// for production)
        let ws_url = if backend_url.starts_with("https://") {
            backend_url.replace("https://", "wss://")
        } else if backend_url.starts_with("http://") {
            // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
            backend_url.replace("http://", "ws://")
        } else {
            // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
            format!("ws://{}", backend_url)
        };

        let ws_url = format!("{}/ws/voice-chat", ws_url);
        info!(url = %ws_url, "Connecting to voice chat WebSocket");

        let (ws_stream, response) = connect_async(&ws_url)
            .await
            .with_context(|| format!("Failed to connect to {}", ws_url))?;

        debug!(status = ?response.status(), "WebSocket connection established");

        let (sink, stream) = ws_stream.split();
        let sink = Arc::new(Mutex::new(sink));

        // Send connected event
        if event_sender.send(VoiceChatEvent::Connected).is_err() {
            warn!("Event receiver dropped before connection event could be sent");
        }

        // Spawn receiver task
        let receiver_handle = tokio::spawn(Self::receive_loop(stream, event_sender));

        Ok(Self {
            sink,
            _receiver_handle: receiver_handle,
        })
    }

    /// Receiver loop that processes incoming WebSocket messages.
    async fn receive_loop(
        mut stream: futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        event_sender: Sender<VoiceChatEvent>,
    ) {
        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    debug!(message = %text, "Received WebSocket message");
                    if let Err(e) = Self::handle_message(&text, &event_sender) {
                        error!(error = %e, "Failed to handle message");
                    }
                }
                Ok(Message::Close(frame)) => {
                    info!(frame = ?frame, "WebSocket connection closed by server");
                    let _ = event_sender.send(VoiceChatEvent::Disconnected);
                    break;
                }
                Ok(Message::Ping(data)) => {
                    debug!("Received ping, pong will be sent automatically");
                    // tungstenite handles pong automatically
                    drop(data);
                }
                Ok(Message::Pong(_)) => {
                    debug!("Received pong");
                }
                Ok(Message::Binary(data)) => {
                    warn!(len = data.len(), "Received unexpected binary message");
                }
                Ok(Message::Frame(_)) => {
                    // Raw frame, usually not received at this level
                }
                Err(e) => {
                    error!(error = %e, "WebSocket error");
                    let _ = event_sender.send(VoiceChatEvent::Error(e.to_string()));
                    break;
                }
            }
        }

        let _ = event_sender.send(VoiceChatEvent::Disconnected);
    }

    /// Handle a single text message from the server.
    fn handle_message(text: &str, event_sender: &Sender<VoiceChatEvent>) -> Result<()> {
        let msg: ServerMessage =
            serde_json::from_str(text).with_context(|| format!("Failed to parse: {}", text))?;

        let event = match msg {
            ServerMessage::Hello { protocol } => {
                info!(protocol = %protocol, "Server hello received");
                return Ok(()); // No event to send for hello
            }
            ServerMessage::Ack { received_bytes } => {
                debug!(bytes = received_bytes, "Server acknowledged audio");
                return Ok(()); // No event to send for ack
            }
            ServerMessage::Transcript { text } => VoiceChatEvent::Transcript(text),
            ServerMessage::SentenceComplete { text } => VoiceChatEvent::SentenceComplete(text),
            ServerMessage::LlmDelta { delta } => VoiceChatEvent::LlmDelta(delta),
            ServerMessage::LlmDone { text, response_id } => {
                VoiceChatEvent::LlmDone { text, response_id }
            }
            ServerMessage::Error { message } => VoiceChatEvent::Error(message),
        };

        event_sender
            .send(event)
            .map_err(|_| anyhow::anyhow!("Event receiver dropped"))?;

        Ok(())
    }

    /// Send an audio chunk to the server.
    ///
    /// # Arguments
    /// * `chunk` - Raw audio bytes (typically PCM).
    /// * `sample_rate` - Audio sample rate in Hz.
    /// * `is_last` - Whether this is the last chunk in the stream.
    pub async fn send_audio_chunk(
        &self,
        chunk: &[u8],
        sample_rate: u32,
        is_last: bool,
    ) -> Result<()> {
        let audio_base64 = BASE64_STANDARD.encode(chunk);

        let msg = ClientMessage::Chunk {
            audio_base64,
            sample_rate,
            last: is_last,
        };

        self.send_message(&msg).await
    }

    /// Signal the end of the audio stream.
    pub async fn send_end(&self) -> Result<()> {
        self.send_message(&ClientMessage::End).await
    }

    /// Flush the transcription buffer.
    pub async fn flush(&self) -> Result<()> {
        self.send_message(&ClientMessage::Flush).await
    }

    /// Set the language for transcription.
    ///
    /// # Arguments
    /// * `lang` - Language code (e.g., "pl", "en").
    pub async fn set_language(&self, lang: &str) -> Result<()> {
        let msg = ClientMessage::Set {
            language: lang.to_string(),
        };
        self.send_message(&msg).await
    }

    /// Reset the session.
    pub async fn reset(&self) -> Result<()> {
        self.send_message(&ClientMessage::Reset).await
    }

    /// Send a message to the server.
    async fn send_message(&self, msg: &ClientMessage) -> Result<()> {
        let json = serde_json::to_string(msg).context("Failed to serialize message")?;
        debug!(message = %json, "Sending WebSocket message");

        let mut sink = self.sink.lock().await;
        sink.send(Message::Text(json.into()))
            .await
            .context("Failed to send WebSocket message")?;

        Ok(())
    }

    /// Close the WebSocket connection gracefully.
    pub async fn close(&self) -> Result<()> {
        let mut sink = self.sink.lock().await;
        sink.close().await.context("Failed to close WebSocket")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_serialization() {
        let msg = ClientMessage::Chunk {
            audio_base64: "SGVsbG8=".to_string(),
            sample_rate: 16000,
            last: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"chunk\""));
        assert!(json.contains("\"audio_base64\":\"SGVsbG8=\""));
        assert!(json.contains("\"sample_rate\":16000"));
        assert!(json.contains("\"last\":false"));
    }

    #[test]
    fn test_server_message_deserialization() {
        let json = r#"{"type":"transcript","text":"Hello world"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::Transcript { text } => assert_eq!(text, "Hello world"),
            _ => panic!("Expected Transcript"),
        }

        let json = r#"{"type":"sentence.complete","text":"Complete sentence."}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::SentenceComplete { text } => assert_eq!(text, "Complete sentence."),
            _ => panic!("Expected SentenceComplete"),
        }

        let json = r#"{"type":"llm.delta","delta":"token"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::LlmDelta { delta } => assert_eq!(delta, "token"),
            _ => panic!("Expected LlmDelta"),
        }

        let json = r#"{"type":"llm.done","text":"Full response","response_id":"abc123"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::LlmDone { text, response_id } => {
                assert_eq!(text, "Full response");
                assert_eq!(response_id, "abc123");
            }
            _ => panic!("Expected LlmDone"),
        }
    }

    #[test]
    fn test_base64_encoding() {
        let audio_bytes = b"test audio data";
        let encoded = BASE64_STANDARD.encode(audio_bytes);
        assert_eq!(encoded, "dGVzdCBhdWRpbyBkYXRh");
    }

    #[test]
    fn test_client_message_end_serialization() {
        let msg = ClientMessage::End;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"end"}"#);
    }

    #[test]
    fn test_client_message_flush_serialization() {
        let msg = ClientMessage::Flush;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"flush"}"#);
    }

    #[test]
    fn test_client_message_set_serialization() {
        let msg = ClientMessage::Set {
            language: "pl".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"set""#));
        assert!(json.contains(r#""language":"pl""#));
    }

    #[test]
    fn test_client_message_reset_serialization() {
        let msg = ClientMessage::Reset;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"reset"}"#);
    }

    #[test]
    fn test_server_message_hello() {
        let json = r#"{"type":"hello","protocol":"voice-chat-v1"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::Hello { protocol } => assert_eq!(protocol, "voice-chat-v1"),
            _ => panic!("Expected Hello"),
        }
    }

    #[test]
    fn test_server_message_ack() {
        let json = r#"{"type":"ack","received_bytes":1024}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::Ack { received_bytes } => assert_eq!(received_bytes, 1024),
            _ => panic!("Expected Ack"),
        }
    }

    #[test]
    fn test_server_message_error() {
        let json = r#"{"type":"error","message":"Connection timeout"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::Error { message } => assert_eq!(message, "Connection timeout"),
            _ => panic!("Expected Error"),
        }
    }

    #[test]
    fn test_voice_chat_event_clone() {
        let event = VoiceChatEvent::LlmDelta("token".to_string());
        let cloned = event.clone();
        match cloned {
            VoiceChatEvent::LlmDelta(delta) => assert_eq!(delta, "token"),
            _ => panic!("Expected LlmDelta"),
        }
    }

    #[test]
    fn test_voice_chat_event_debug() {
        let event = VoiceChatEvent::Connected;
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("Connected"));
    }

    #[test]
    fn test_chunk_with_last_flag() {
        let msg = ClientMessage::Chunk {
            audio_base64: "YXVkaW8=".to_string(),
            sample_rate: 48000,
            last: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"last\":true"));
        assert!(json.contains("\"sample_rate\":48000"));
    }
}
