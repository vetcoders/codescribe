use crate::audio::recorder::{Recorder, RecorderConfig};
use crate::stream_postprocess::StreamPostProcessor;
use crate::whisper::append_with_overlap_dedup;
use crate::whisper::singleton::engine as get_engine;
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;
use std::{fs::OpenOptions, io::Write, path::Path};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

const DEFAULT_CHUNK_DURATION_SEC: f32 = 15.0;
const OVERLAP_SEC: f32 = 2.0; // Overlap for context

pub type StreamDeltaCallback = Arc<dyn Fn(&str) + Send + Sync>;

pub struct StreamingRecorder {
    pub recorder: Recorder,
    transcript_buffer: Arc<Mutex<String>>,
    transcription_handle: Option<JoinHandle<()>>,
    sample_rate: u32,
    delta_callback: Option<StreamDeltaCallback>,
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
            delta_callback: None,
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
            delta_callback: None,
        })
    }

    pub fn set_delta_callback(&mut self, callback: Option<StreamDeltaCallback>) {
        self.delta_callback = callback;
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
        let stream_log_path = stream_log_path();
        let delta_callback = self.delta_callback.clone();
        self.transcription_handle = Some(tokio::spawn(async move {
            transcription_worker(
                rx,
                transcript_buffer,
                actual_sample_rate,
                language,
                postprocessor,
                delta_callback,
                stream_log_path,
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
    delta_callback: Option<StreamDeltaCallback>,
    stream_log_path: Option<std::path::PathBuf>,
) {
    info!("Transcription worker started");

    let mut pending_samples: Vec<f32> = Vec::new();
    let chunk_duration_sec = stream_chunk_duration_sec();
    let overlap_sec = stream_overlap_sec(chunk_duration_sec);
    let chunk_limit = (sample_rate as f32 * chunk_duration_sec) as usize;
    let overlap_size = (sample_rate as f32 * overlap_sec) as usize;

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
                delta_callback.as_ref(),
                stream_log_path.as_deref(),
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
            delta_callback.as_ref(),
            stream_log_path.as_deref(),
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
    delta_callback: Option<&StreamDeltaCallback>,
    stream_log_path: Option<&Path>,
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
                    let before_len = buffer.len();
                    append_with_overlap_dedup(&mut buffer, &cleaned);
                    if let Some(delta) = buffer.get(before_len..)
                        && !delta.trim().is_empty()
                    {
                        if let Some(callback) = delta_callback {
                            callback(delta);
                        }

                        // Log to file if enabled
                        if let Some(path) = stream_log_path {
                            let _ = append_to_stream_log(path, delta);
                        }
                    }
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

fn stream_log_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("CODESCRIBE_STREAM_LOG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(std::path::PathBuf::from(trimmed));
        }
    }

    if env_bool("CODESCRIBE_STREAM_LOG") {
        let root = crate::config::Config::config_dir();
        return Some(root.join("stream.log"));
    }

    None
}

fn append_to_stream_log(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", text.trim_end())?;
    Ok(())
}

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

fn stream_chunk_duration_sec() -> f32 {
    env_f32("CODESCRIBE_STREAM_CHUNK_SEC", DEFAULT_CHUNK_DURATION_SEC).clamp(0.5, 30.0)
}

fn stream_overlap_sec(chunk_duration_sec: f32) -> f32 {
    OVERLAP_SEC.min(chunk_duration_sec * 0.8)
}

pub fn transcribe_streaming_samples(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    mut postprocessor: Option<&mut StreamPostProcessor>,
) -> Result<String> {
    if samples.is_empty() {
        return Ok(String::new());
    }

    let chunk_duration_sec = stream_chunk_duration_sec();
    let overlap_sec = stream_overlap_sec(chunk_duration_sec);
    let chunk_limit = (sample_rate as f32 * chunk_duration_sec) as usize;
    let overlap_size = (sample_rate as f32 * overlap_sec) as usize;
    let step = chunk_limit.saturating_sub(overlap_size).max(1);

    let engine_mutex = get_engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Lock error: {}", e))?;

    let mut out = String::new();
    let mut offset = 0usize;

    while offset < samples.len() {
        let end = (offset + chunk_limit).min(samples.len());
        let chunk = &samples[offset..end];
        let text = engine.transcribe_with_language(chunk, sample_rate, language)?;

        if let Some(processor) = postprocessor.as_deref_mut() {
            if let Some(cleaned) = processor.process(&text) {
                append_with_overlap_dedup(&mut out, &cleaned);
            }
        } else {
            append_with_overlap_dedup(&mut out, &text);
        }

        if end == samples.len() {
            break;
        }
        offset = offset.saturating_add(step);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::load_audio_file;
    use crate::whisper;
    use serial_test::serial;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    #[serial]
    fn test_stream_postprocess_corpus_pairs() {
        if !env_bool("CODESCRIBE_E2E_CORPUS") {
            eprintln!("Skipping corpus E2E (set CODESCRIBE_E2E_CORPUS=1 to enable)");
            return;
        }

        let corpus_dir = corpus_root();
        let date_filter = std::env::var("CODESCRIBE_E2E_CORPUS_DATE").ok();
        let limit = env_usize("CODESCRIBE_E2E_CORPUS_LIMIT", 3);
        let max_regression = env_f32("CODESCRIBE_E2E_CORPUS_MAX_REGRESSION", 0.05);

        let pairs = collect_pairs(&corpus_dir, date_filter.as_deref(), limit);
        if pairs.is_empty() {
            eprintln!("No WAV+TXT pairs found in {}", corpus_dir.to_string_lossy());
            return;
        }

        whisper::init().expect("Failed to init Whisper");
        let language = std::env::var("CODESCRIBE_E2E_CORPUS_LANGUAGE").ok();
        let mut failures = Vec::new();
        let mut total_raw_wer = 0.0;
        let mut total_post_wer = 0.0;
        let mut total_raw_cer = 0.0;
        let mut total_post_cer = 0.0;
        let mut processed = 0usize;

        for (wav_path, txt_path) in pairs {
            let reference = fs::read_to_string(&txt_path)
                .unwrap_or_else(|_| String::new())
                .trim()
                .to_string();
            if reference.is_empty() {
                eprintln!("Skipping empty reference: {}", txt_path.display());
                continue;
            }

            let (samples, sample_rate) = load_audio_file(&wav_path).expect("Failed to load audio");

            let raw =
                transcribe_streaming_samples(&samples, sample_rate, language.as_deref(), None)
                    .expect("Raw streaming transcription failed");
            let mut postprocessor = StreamPostProcessor::new();
            let post = transcribe_streaming_samples(
                &samples,
                sample_rate,
                language.as_deref(),
                Some(&mut postprocessor),
            )
            .expect("Post streaming transcription failed");

            let (ref_tokens, ref_norm) = normalize_for_eval(&reference);
            let (raw_tokens, raw_norm) = normalize_for_eval(&raw);
            let (post_tokens, post_norm) = normalize_for_eval(&post);

            let wer_raw = word_error_rate(&ref_tokens, &raw_tokens);
            let wer_post = word_error_rate(&ref_tokens, &post_tokens);
            let cer_raw = char_error_rate(&ref_norm, &raw_norm);
            let cer_post = char_error_rate(&ref_norm, &post_norm);

            processed += 1;
            total_raw_wer += wer_raw;
            total_post_wer += wer_post;
            total_raw_cer += cer_raw;
            total_post_cer += cer_post;

            println!(
                "Corpus: {}\n  WER raw={:.3} post={:.3} (Δ={:.3})\n  CER raw={:.3} post={:.3} (Δ={:.3})",
                wav_path.file_name().unwrap_or_default().to_string_lossy(),
                wer_raw,
                wer_post,
                wer_post - wer_raw,
                cer_raw,
                cer_post,
                cer_post - cer_raw,
            );

            if wer_post > wer_raw + max_regression {
                failures.push(format!(
                    "{}: WER regression {:.3} > {:.3}",
                    wav_path.display(),
                    wer_post - wer_raw,
                    max_regression
                ));
            }
        }

        if processed > 0 {
            let denom = processed as f32;
            let avg_raw_wer = total_raw_wer / denom;
            let avg_post_wer = total_post_wer / denom;
            let avg_raw_cer = total_raw_cer / denom;
            let avg_post_cer = total_post_cer / denom;

            println!(
                "Average WER raw={:.3} post={:.3} | CER raw={:.3} post={:.3}",
                avg_raw_wer, avg_post_wer, avg_raw_cer, avg_post_cer
            );
        }

        if !failures.is_empty() {
            panic!(
                "Corpus postprocess regressions detected:\n{}",
                failures.join("\n")
            );
        }
    }

    fn env_bool(key: &str) -> bool {
        std::env::var(key)
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    fn env_f32(key: &str, default: f32) -> f32 {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(default)
    }

    fn env_usize(key: &str, default: usize) -> usize {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default)
    }

    fn corpus_root() -> PathBuf {
        if let Ok(dir) = std::env::var("CODESCRIBE_E2E_CORPUS_DIR") {
            return PathBuf::from(shellexpand::tilde(&dir).into_owned());
        }

        crate::config::Config::config_dir().join("transcriptions")
    }

    fn collect_pairs(
        root: &Path,
        date_filter: Option<&str>,
        limit: usize,
    ) -> Vec<(PathBuf, PathBuf)> {
        let mut pairs = Vec::new();
        if !root.exists() {
            return pairs;
        }

        let mut subdirs = Vec::new();
        if let Some(date) = date_filter {
            let dir = root.join(date);
            if dir.exists() {
                subdirs.push(dir);
            }
        } else if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    subdirs.push(path);
                }
            }
        }

        subdirs.sort();

        for dir in subdirs {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut wavs = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("wav") {
                    wavs.push(path);
                }
            }

            wavs.sort();
            for wav in wavs {
                let stem = match wav.file_stem().and_then(|s| s.to_str()) {
                    Some(stem) => stem,
                    None => continue,
                };
                let txt = wav.with_file_name(format!("{stem}.txt"));
                if txt.exists() {
                    pairs.push((wav, txt));
                }
            }
        }

        if limit > 0 && pairs.len() > limit {
            let start = pairs.len() - limit;
            pairs = pairs[start..].to_vec();
        }

        pairs
    }

    fn normalize_for_eval(text: &str) -> (Vec<String>, String) {
        let mut normalized = String::with_capacity(text.len());
        for ch in text.to_lowercase().chars() {
            if ch.is_alphanumeric() || ch.is_whitespace() {
                normalized.push(ch);
            } else {
                normalized.push(' ');
            }
        }
        let tokens: Vec<String> = normalized
            .split_whitespace()
            .map(|t| t.to_string())
            .collect();
        let normalized = tokens.join(" ");
        (tokens, normalized)
    }

    fn word_error_rate(reference: &[String], hypothesis: &[String]) -> f32 {
        let dist = levenshtein(reference, hypothesis);
        let denom = reference.len().max(1) as f32;
        dist as f32 / denom
    }

    fn char_error_rate(reference: &str, hypothesis: &str) -> f32 {
        let ref_chars: Vec<char> = reference.chars().collect();
        let hyp_chars: Vec<char> = hypothesis.chars().collect();
        let dist = levenshtein(&ref_chars, &hyp_chars);
        let denom = ref_chars.len().max(1) as f32;
        dist as f32 / denom
    }

    fn levenshtein<T: Eq>(a: &[T], b: &[T]) -> usize {
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        let mut cur = vec![0usize; b.len() + 1];

        for (i, item_a) in a.iter().enumerate() {
            cur[0] = i + 1;
            for (j, item_b) in b.iter().enumerate() {
                let cost = if item_a == item_b { 0 } else { 1 };
                cur[j + 1] =
                    std::cmp::min(std::cmp::min(prev[j + 1] + 1, cur[j] + 1), prev[j] + cost);
            }
            prev.clone_from(&cur);
        }

        prev[b.len()]
    }
}
