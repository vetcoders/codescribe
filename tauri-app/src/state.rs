//! Application state for Tauri backend
//!
//! Simplified state - most operations go through IPC to CLI.
//!
//! Created by M&K (c)2026 VetCoders

#[cfg(not(target_arch = "wasm32"))]
use codescribe_core::Recorder;
#[cfg(not(target_arch = "wasm32"))]
use codescribe_core::config::Config;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex as TokioMutex;

/// Recording state for standalone mode (when CLI isn't running)
#[cfg(not(target_arch = "wasm32"))]
pub struct RecordingState {
    pub recorder: Option<Recorder>,
    pub is_recording: bool,
    pub via_ipc: bool,
}

/// Application state
#[cfg(not(target_arch = "wasm32"))]
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub recording: Arc<TokioMutex<RecordingState>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            recording: Arc::clone(&self.recording),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AppState {
    pub fn new() -> Result<Self, String> {
        let config = Config::load();
        Ok(Self {
            config: Arc::new(Mutex::new(config)),
            recording: Arc::new(TokioMutex::new(RecordingState {
                recorder: None,
                is_recording: false,
                via_ipc: false,
            })),
        })
    }
}
