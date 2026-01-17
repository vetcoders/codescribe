use crate::audio::recorder::{Recorder, RecorderConfig};
use crate::stream_postprocess::StreamPostProcessor;
use crate::whisper::append_with_overlap_dedup;
use crate::whisper::singleton::engine as get_engine;
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

const CHUNK_DURATION_SEC: f32 = 15.0;
const OVERLAP_SEC: f32 = 2.0; // Overlap for context

pub struct StreamingRecorder {
    pub recorder: Recorder,
    transcript_buffer: Arc<Mutex<String>>,
    transcription_handle: Option<JoinHandle<()>>,
    sample_rate: u32,
}

impl StreamingRecorder {
    pub fn new() -> Result<Self> {
        let recorder = Recorder::new()?;
        let sample_rate = recorder.config.sample_rate;

        Ok(Self {
            recorder,
            transcript_buffer: Arc::new(Mutex::new(String::new())),
            transcription_handle: None,
            sample_rate,
        })
    }

    pub fn with_config(config: RecorderConfig) -> Result<Self> {
        let sample_rate = config.sample_rate;
        let recorder = Recorder::with_config(config)?;

        Ok(Self {
            recorder,
            transcript_buffer: Arc::new(Mutex::new(String::new())),
            transcription_handle: None,
            sample_rate,
        })
    }

    pub async fn start(&mut self, language: Option<String>) -> Result<()> {
        // Clear previous transcript
        *self.transcript_buffer.lock().await = String::new();

        // Create channel for audio chunks
        // Buffer size: enough to hold a few seconds if worker is slow
        let (tx, rx) = mpsc::channel::<Vec<f32>>(500);

        // Setup callback to send audio data
        // Note: try_send to avoid blocking audio thread
        self.recorder.set_callback(Box::new(move |data| {
            if let Err(_e) = tx.try_send(data.to_vec()) {
                // If channel is full, we drop audio (better than blocking)
                // But we should log occasionally?
                // For now just ignore or print to stderr if needed, but avoid spamming logs
            }
        }));

        // Start the actual audio stream first, so we know the *real* sample rate (often 48kHz).
        self.recorder.start().await?;

        // Update sample rate to the one used by the input stream.
        // This is critical: we must pass the correct `sample_rate` to Whisper so it can resample.
        let actual_sample_rate = self.recorder.actual_sample_rate();
        if actual_sample_rate != self.sample_rate {
            info!(
                "StreamingRecorder sample_rate updated: config={}Hz -> actual={}Hz",
                self.sample_rate, actual_sample_rate
            );
            self.sample_rate = actual_sample_rate;
        } else {
            debug!("StreamingRecorder sample_rate: {}Hz", actual_sample_rate);
        }

        // Start transcription worker (after we know the real sample rate)
        let transcript_buffer = self.transcript_buffer.clone();
        let postprocessor = StreamPostProcessor::new();
        self.transcription_handle = Some(tokio::spawn(async move {
            transcription_worker(
                rx,
                transcript_buffer,
                actual_sample_rate,
                language,
                postprocessor,
            )
            .await;
        }));

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(String, Option<std::path::PathBuf>)> {
        info!("Stopping streaming recorder...");

        // 1. Stop recording (drops callback and sender)
        let audio_path = self.recorder.stop().await?;

        // 2. Wait for worker to finish processing remaining chunks
        if let Some(handle) = self.transcription_handle.take() {
            debug!("Waiting for transcription worker to finish...");
            handle.await.context("Transcription worker failed")?;
        }

        // 3. Return collected transcript
        let transcript = self.transcript_buffer.lock().await.clone();
        Ok((transcript, audio_path))
    }
}

async fn transcription_worker(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    transcript_buffer: Arc<Mutex<String>>,
    sample_rate: u32,
    language: Option<String>,
    mut postprocessor: StreamPostProcessor,
) {
    info!("Transcription worker started");

    let mut pending_samples: Vec<f32> = Vec::new();
    let chunk_limit = (sample_rate as f32 * CHUNK_DURATION_SEC) as usize;
    let overlap_size = (sample_rate as f32 * OVERLAP_SEC) as usize;

    // We keep track of how many samples we've processed to know when to overlap
    // Actually, we just keep the last samples in pending_samples?
    // No, pending_samples grows. When it hits limit, we transcribe.
    // Then we keep the tail as the new pending_samples.

    while let Some(mut data) = chunk_receiver.recv().await {
        pending_samples.append(&mut data);

        if pending_samples.len() >= chunk_limit {
            process_chunk(
                &pending_samples,
                &transcript_buffer,
                sample_rate,
                language.as_deref(),
                &mut postprocessor,
            )
            .await;

            // Keep overlap for next chunk
            if pending_samples.len() > overlap_size {
                let start_idx = pending_samples.len() - overlap_size;
                pending_samples = pending_samples[start_idx..].to_vec();
            } else {
                // Should not happen if chunk_limit > overlap_size
                pending_samples.clear();
            }
        }
    }

    // Process remaining samples (final chunk)
    if !pending_samples.is_empty() {
        debug!("Processing final chunk ({} samples)", pending_samples.len());
        process_chunk(
            &pending_samples,
            &transcript_buffer,
            sample_rate,
            language.as_deref(),
            &mut postprocessor,
        )
        .await;
    }

    info!("Transcription worker finished");
}

async fn process_chunk(
    samples: &[f32],
    transcript_buffer: &Arc<Mutex<String>>,
    sample_rate: u32,
    language: Option<&str>,
    postprocessor: &mut StreamPostProcessor,
) {
    if samples.is_empty() {
        return;
    }

    let samples_owned = samples.to_vec();
    let lang_owned = language.map(String::from);

    // Run in blocking task
    let result = tokio::task::spawn_blocking(move || {
        let engine_mutex = match get_engine() {
            Ok(m) => m,
            Err(e) => return Err(anyhow!("Engine error: {}", e)),
        };

        let mut engine_guard = match engine_mutex.lock() {
            Ok(g) => g,
            Err(e) => return Err(anyhow!("Lock error: {}", e)),
        };

        // If sample_rate is not 16k, engine handles resampling?
        // transcribe_samples_16k expects 16k.
        // But our Recorder is configured for 16k (SAMPLE_RATE constant).
        // However, Recorder might use native rate.
        // Recorder::start() sets actual_sample_rate.
        // If actual_sample_rate != 16k, we need to resample.
        // Current implementation passes raw samples.
        // transcribe_samples_16k assumes 16k.
        // transcribe_with_language handles resampling.

        // Wait, engine.transcribe_samples_16k is specific.
        // engine.transcribe_with_language(audio, sample_rate, language) handles everything.
        // Let's use that one to be safe, or check if we need 16k.

        // The plan says "transcribe_samples_16k() - transcribes raw f32, zero I/O".
        // If we use transcribe_with_language, it calls transcribe_long_with_language -> detect_language -> ...
        // transcribe_samples_16k is lower level.

        // If sample_rate is 16000, we can use transcribe_samples_16k directly?
        // Yes, but we should be robust.
        // Let's use transcribe_with_language which handles resampling if needed.
        // It's safer.

        engine_guard.transcribe_with_language(&samples_owned, sample_rate, lang_owned.as_deref())
    })
    .await;

    match result {
        Ok(Ok(text)) => {
            if !text.trim().is_empty() {
                debug!("Chunk transcribed: '{}'", text.trim());
                if let Some(cleaned) = postprocessor.process(&text) {
                    let mut buffer = transcript_buffer.lock().await;
                    append_with_overlap_dedup(&mut buffer, &cleaned);
                } else {
                    debug!("Stream postprocessor dropped chunk");
                }
            }
        }
        Ok(Err(e)) => {
            error!("Chunk transcription failed: {}", e);
        }
        Err(e) => {
            error!("Transcription task join error: {}", e);
        }
    }
}
