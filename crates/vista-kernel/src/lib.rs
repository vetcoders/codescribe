#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

pub use qube_support::safe_path;

pub mod audio {
    pub use qube_audio::audio::recorder::{Recorder, RecorderConfig, RecorderDiagnostics};
    pub use qube_audio::audio::{load_audio_file, resample_to_16k};

    pub mod recorder {
        pub use qube_audio::audio::recorder::*;
    }

    pub mod streaming_recorder {
        use std::path::PathBuf;
        use std::sync::Arc;

        use anyhow::Result;

        use crate::pipeline::contracts::EventSink;

        #[derive(Default)]
        pub struct StreamingRecorder {
            event_sink: Option<Arc<dyn EventSink>>,
            language: Option<String>,
        }

        impl StreamingRecorder {
            pub fn new() -> Result<Self> {
                Ok(Self::default())
            }

            pub fn set_event_sink(&mut self, sink: Option<Arc<dyn EventSink>>) {
                self.event_sink = sink;
            }

            pub async fn start_event_session(&mut self, language: Option<String>) -> Result<()> {
                self.language = language;
                Ok(())
            }

            pub async fn stop(&mut self) -> Result<(String, Option<PathBuf>)> {
                Ok((String::new(), None))
            }
        }
    }
}

pub mod config {
    pub use qube_support::config::*;
}

pub mod pipeline {
    pub mod contracts {
        pub use qube_stt::pipeline::contracts::*;
    }

    pub mod sinks {
        pub use qube_ws::pipeline::sinks::*;
    }

    pub mod stream_postprocess {
        pub use qube_ws::pipeline::stream_postprocess::*;
    }

    pub mod streaming {
        pub use qube_ws::pipeline::streaming::*;
    }

    pub use qube_ws::pipeline::{
        CollectorEventSink, DeltaSinkAdapter, DropKind, EngineEvent, EventSink, FanoutEventSink,
    };
}

pub mod stt {
    pub use qube_stt::stt::*;
}

pub mod whisper {
    pub use qube_stt::stt::whisper::*;
}

pub mod vad {
    pub use qube_audio::vad::*;
}

pub mod vad_api {
    pub use qube_audio::vad::*;
}

pub mod stream_postprocess {
    pub use qube_ws::pipeline::stream_postprocess::*;
}

pub mod embedder {
    use super::*;

    pub const EMBEDDING_DIM: usize = 384;

    static INITIALIZED: AtomicBool = AtomicBool::new(false);

    #[derive(Debug, Clone)]
    pub struct EmbeddedModel;

    impl EmbeddedModel {
        pub fn total_size(&self) -> usize {
            0
        }
    }

    pub mod embedded {
        use super::EmbeddedModel;

        pub fn is_embedded_available() -> bool {
            false
        }

        pub fn get_embedded_data() -> Option<EmbeddedModel> {
            None
        }
    }

    pub fn init() -> anyhow::Result<()> {
        INITIALIZED.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn is_initialized() -> bool {
        INITIALIZED.load(Ordering::SeqCst)
    }

    pub fn embed(text: &str) -> anyhow::Result<Vec<f32>> {
        let mut out = vec![0.0f32; EMBEDDING_DIM];
        for token in text.split_whitespace() {
            let mut hasher = DefaultHasher::new();
            token.to_ascii_lowercase().hash(&mut hasher);
            let idx = (hasher.finish() as usize) % EMBEDDING_DIM;
            out[idx] += 1.0;
        }

        let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut out {
                *value /= norm;
            }
        }

        Ok(out)
    }

    pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
        let len = a.len().min(b.len());
        let dot = a
            .iter()
            .zip(b.iter())
            .take(len)
            .map(|(x, y)| x * y)
            .sum::<f32>();
        let norm_a = a.iter().take(len).map(|v| v * v).sum::<f32>().sqrt();
        let norm_b = b.iter().take(len).map(|v| v * v).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            (dot / (norm_a * norm_b)).clamp(0.0, 1.0)
        }
    }
}

pub mod tts {
    use std::sync::atomic::{AtomicBool, Ordering};

    use anyhow::Result;

    static INITIALIZED: AtomicBool = AtomicBool::new(false);
    const SAMPLE_RATE: u32 = 24_000;

    #[derive(Debug, Clone)]
    pub struct EmbeddedModel;

    impl EmbeddedModel {
        pub fn total_size(&self) -> usize {
            0
        }
    }

    pub mod embedded {
        use super::EmbeddedModel;

        pub fn is_embedded_available() -> bool {
            false
        }

        pub fn get_embedded_data() -> Option<EmbeddedModel> {
            None
        }
    }

    pub struct AudioPlayer;

    impl AudioPlayer {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }

        pub fn play_blocking(&self, _audio: &[f32], _sample_rate: u32) -> Result<()> {
            Ok(())
        }
    }

    pub fn init() -> Result<()> {
        INITIALIZED.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn synthesize(text: &str) -> Result<Vec<f32>> {
        if !INITIALIZED.load(Ordering::SeqCst) {
            init()?;
        }

        let seconds = (text.chars().count().max(1) as f32 * 0.06).max(0.5);
        let total = (seconds * SAMPLE_RATE as f32) as usize;
        let freq = 220.0f32;

        Ok((0..total)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                (2.0 * std::f32::consts::PI * freq * t).sin() * 0.15
            })
            .collect())
    }
}

pub use audio::recorder::{Recorder, RecorderConfig, RecorderDiagnostics};
pub use llm::{ai_formatting, client};
pub use quality::{quality_loop, quality_report};
pub use server::{LocalTranscriptionBackend, ServerConfig, ServerHandle, TranscribeResponse};

pub mod llm;
pub mod quality;
pub mod server;
pub mod state;
pub mod status;

pub fn should_show_onboarding() -> bool {
    let config_dir = config::Config::config_dir();
    let setup_done = config_dir.join("setup_done");
    if setup_done.exists() {
        return false;
    }

    let onboarding_done = config_dir.join("onboarding_done");
    let bootstrap_done = config_dir.join("bootstrap_done");
    if onboarding_done.exists() && bootstrap_done.exists() {
        let _ = fs::write(&setup_done, "done");
        return false;
    }

    true
}
