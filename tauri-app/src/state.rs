#[cfg(not(target_arch = "wasm32"))]
use codescribe::audio::Recorder;
#[cfg(not(target_arch = "wasm32"))]
use codescribe::config::Config;
#[cfg(not(target_arch = "wasm32"))]
use codescribe::models::ModelManager;
#[cfg(not(target_arch = "wasm32"))]
use codescribe::whisper::LocalWhisperEngine;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex as TokioMutex;

#[cfg(not(target_arch = "wasm32"))]
pub struct SttState {
    pub loaded_model: Option<String>,
    pub engine: Option<LocalWhisperEngine>,
}

#[cfg(not(target_arch = "wasm32"))]
pub struct RecordingState {
    pub recorder: Option<Recorder>,
    pub is_recording: bool,
}

#[cfg(not(target_arch = "wasm32"))]
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub model_manager: Arc<ModelManager>,
    pub stt: Arc<Mutex<SttState>>,
    pub recording: Arc<TokioMutex<RecordingState>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            model_manager: Arc::clone(&self.model_manager),
            stt: Arc::clone(&self.stt),
            recording: Arc::clone(&self.recording),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AppState {
    pub fn new() -> Result<Self, String> {
        let config = Config::load();
        let model_manager = ModelManager::new().map_err(|e| e.to_string())?;
        Ok(Self {
            config: Arc::new(Mutex::new(config)),
            model_manager: Arc::new(model_manager),
            stt: Arc::new(Mutex::new(SttState {
                loaded_model: None,
                engine: None,
            })),
            recording: Arc::new(TokioMutex::new(RecordingState {
                recorder: None,
                is_recording: false,
            })),
        })
    }
}
