use axum::{
    Router,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

use crate::pipeline::contracts::{EngineEvent, EventSink};
use crate::pipeline::streaming::{SessionConfig, transcription_session};

pub async fn start_server(port: u16) -> anyhow::Result<()> {
    // Force Apple STT backend for Qube Protocol as requested
    unsafe {
        std::env::set_var("CODESCRIBE_STT_ENGINE", "apple");
    }

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/", get(|| async { "Libraxis Qube Server Running" }));

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Libraxis Qube WebSocket server listening on {}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

struct WebSocketEventSink {
    tx: mpsc::UnboundedSender<String>,
}

impl EventSink for WebSocketEventSink {
    fn on_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::Preview { text, .. } => {
                if !text.is_empty() {
                    let _ = self.tx.send(format!("<speak>{}</speak>", text));
                }
            }
            EngineEvent::UtteranceFinal { text, .. } => {
                if !text.is_empty() {
                    let _ = self.tx.send(format!("<speak>{}</speak>", text));
                }
            }
            _ => {}
        }
    }
}

async fn handle_socket(socket: WebSocket) {
    info!("Client connected to Qube WebSocket");

    let (mut sender, mut receiver) = socket.split();
    let (tx_audio, rx_audio) = mpsc::channel::<Vec<f32>>(100);
    let (tx_tags, mut rx_tags) = mpsc::unbounded_channel::<String>();

    let event_sink = Arc::new(WebSocketEventSink { tx: tx_tags });

    let config = SessionConfig {
        sample_rate: 16000,
        language: Some("pl".to_string()),
        stream_log_path: None,
        utterance_silence_sec: None,
    };

    // Spawn transcription session
    tokio::spawn(async move {
        transcription_session(rx_audio, event_sink, config).await;
        info!("Transcription session ended");
    });

    // Spawn task to send tags back to client
    let mut send_task = tokio::spawn(async move {
        while let Some(tag) = rx_tags.recv().await {
            if sender.send(Message::Text(tag.into())).await.is_err() {
                break;
            }
        }
    });

    // Receive audio chunks from client
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Binary(bytes) = msg {
                // Convert 16-bit PCM little-endian to f32
                let samples: Vec<f32> = bytes
                    .chunks_exact(2)
                    .map(|chunk| {
                        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                        sample as f32 / 32768.0
                    })
                    .collect();

                if tx_audio.send(samples).await.is_err() {
                    break;
                }
            } else if let Message::Close(_) = msg {
                break;
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };

    info!("Client disconnected from Qube WebSocket");
}
