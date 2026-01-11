#[cfg(not(target_arch = "wasm32"))]
use codescribe::config::Config;
#[cfg(not(target_arch = "wasm32"))]
use codescribe::local_stt::LocalWhisperEngine;
#[cfg(not(target_arch = "wasm32"))]
use codescribe::models::ModelManager;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
pub struct SttState {
    pub loaded_model: Option<String>,
    pub engine: Option<LocalWhisperEngine>,
}

#[cfg(not(target_arch = "wasm32"))]
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub model_manager: ModelManager,
    pub stt: Arc<Mutex<SttState>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl AppState {
    pub fn new() -> Result<Self, String> {
        let config = Config::load();
        let model_manager = ModelManager::new().map_err(|e| e.to_string())?;
        Ok(Self {
            config: Arc::new(Mutex::new(config)),
            model_manager,
            stt: Arc::new(Mutex::new(SttState {
                loaded_model: None,
                engine: None,
            })),
        })
    }
}
