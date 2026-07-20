use crate::audio::recorder::{Recorder, RecorderConfig};
use crate::pipeline::contracts::EventSink;
use crate::pipeline::streaming::{SessionConfig, stream_log_path, transcription_session};
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

// Keep enough raw audio queued to survive a cold Whisper load without dropping
// the user's first words. The STT session drains this backlog once the model is ready.
const AUDIO_BACKLOG_CHUNKS: usize = 2048;

pub struct StreamingRecorder {
    pub recorder: Recorder,
    transcript_buffer: Arc<Mutex<String>>,
    transcription_handle: Option<JoinHandle<()>>,
    sample_rate: u32,
    utterance_callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    utterance_silence_sec: Option<f32>,
    /// Counter for audio chunks dropped due to channel backpressure.
    dropped_chunks: Arc<AtomicU64>,
    /// Sink used by `start_event_session`. Caller must configure it explicitly.
    event_sink: Option<Arc<dyn EventSink>>,
    /// Per-block input level tap: receives the RMS of every captured audio
    /// block (linear, 0..~1). Runs on the CoreAudio callback thread — keep it
    /// cheap and non-blocking (a broadcast send, an atomic store).
    level_callback: Option<Arc<dyn Fn(f32) + Send + Sync>>,
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
            utterance_callback: None,
            utterance_silence_sec: None,
            dropped_chunks: Arc::new(AtomicU64::new(0)),
            event_sink: None,
            level_callback: None,
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
            utterance_callback: None,
            utterance_silence_sec: None,
            dropped_chunks: Arc::new(AtomicU64::new(0)),
            event_sink: None,
            level_callback: None,
        })
    }

    pub fn set_utterance_callback(&mut self, callback: Option<Arc<dyn Fn(String) + Send + Sync>>) {
        self.utterance_callback = callback;
    }

    pub fn set_utterance_silence_sec(&mut self, silence_sec: Option<f32>) {
        self.utterance_silence_sec = silence_sec;
    }

    /// Set the per-block input-level tap consumed by UI meters (overlay
    /// waveform). Configure before `start_event_session`; cleared alongside the
    /// other callbacks between sessions.
    pub fn set_level_callback(&mut self, callback: Option<Arc<dyn Fn(f32) + Send + Sync>>) {
        self.level_callback = callback;
    }

    /// Returns a cloned handle to the transcript buffer.
    ///
    /// Used by `ControllerEventRouter` to update the buffer as previews arrive,
    /// so `stop()` returns the accumulated text.
    pub fn transcript_buffer_handle(&self) -> Arc<Mutex<String>> {
        self.transcript_buffer.clone()
    }

    /// Set the event sink for the unified pipeline.
    pub fn set_event_sink(&mut self, sink: Option<Arc<dyn EventSink>>) {
        self.event_sink = sink;
    }

    /// Returns true when the underlying recorder still has an active audio stream.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_active()
    }

    /// Start recording with the new event-based pipeline.
    ///
    /// Uses `transcription_session` which emits `EngineEvent`s to the configured
    /// `event_sink`.
    pub async fn start_event_session(&mut self, language: Option<String>) -> Result<()> {
        let event_sink = self.event_sink.clone().ok_or_else(|| {
            anyhow!(
                "start_event_session requires event_sink (set_event_sink(Some(...)) before start)"
            )
        })?;

        // Clear previous transcript and reset drop counter
        *self.transcript_buffer.lock().await = String::new();
        self.dropped_chunks.store(0, Ordering::Relaxed);

        // Create channel for audio chunks. This is intentionally larger than a
        // normal live queue: cold STT initialization happens behind this buffer.
        let (tx, rx) = mpsc::channel::<Vec<f32>>(AUDIO_BACKLOG_CHUNKS);

        // Setup callback to send audio data
        let dropped = Arc::clone(&self.dropped_chunks);
        let level_callback = self.level_callback.clone();
        self.recorder.set_callback(Box::new(move |data| {
            if let Some(ref level_cb) = level_callback {
                level_cb(block_rms(data));
            }
            if let Err(_e) = tx.try_send(data.to_vec()) {
                let n = dropped.fetch_add(1, Ordering::Relaxed);
                if n == 0 || (n + 1).is_multiple_of(50) {
                    tracing::warn!("Audio callback: channel full, dropped {} chunk(s)", n + 1);
                }
            }
        }));

        // Start actual audio stream
        self.recorder.start().await?;

        // Update sample rate to match real input stream
        let actual_sample_rate = self.recorder.actual_sample_rate();
        if actual_sample_rate != self.sample_rate {
            info!(
                "StreamingRecorder sample_rate updated: config={}Hz -> actual={}Hz",
                self.sample_rate, actual_sample_rate
            );
            self.sample_rate = actual_sample_rate;
        }

        let log_path = stream_log_path();
        let utterance_silence_sec = self.utterance_silence_sec;

        self.transcription_handle = Some(tokio::spawn(async move {
            transcription_session(
                rx,
                event_sink,
                SessionConfig {
                    sample_rate: actual_sample_rate,
                    language,
                    stream_log_path: log_path,
                    utterance_silence_sec,
                },
            )
            .await;
        }));

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(String, Option<std::path::PathBuf>)> {
        info!("Stopping streaming recorder...");

        // Report any dropped audio chunks
        let drops = self.dropped_chunks.load(Ordering::Relaxed);
        if drops > 0 {
            warn!(
                "Recording session: dropped {} audio chunk(s) due to backpressure",
                drops
            );
        }

        // 1. Stop recording (drops callback and sender)
        let audio_path = self.recorder.stop().await?;

        // 2. Wait for worker to finish processing remaining chunks
        if let Some(handle) = self.transcription_handle.take() {
            debug!("Waiting for transcription session task to finish...");
            handle.await.context("Transcription session task failed")?;
        }

        // 3. Drain presentation layer.
        // PresentationEmitter's BufferedEmitter tick loop runs in a separate
        // tokio task. After transcription_session sends Finish, the tick loop
        // needs time to drain queued text into transcript_buffer before we
        // drop the event sink (which aborts the tick loop via Drop).
        if self.event_sink.is_some() {
            let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let snapshot = self.transcript_buffer.lock().await.len();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if self.transcript_buffer.lock().await.len() == snapshot
                    || tokio::time::Instant::now() >= drain_deadline
                {
                    break;
                }
            }
        }
        self.event_sink = None;

        // 4. Return collected transcript
        let transcript = self.transcript_buffer.lock().await.clone();
        Ok((transcript, audio_path))
    }

    #[deprecated(note = "use stop_and_discard_path instead")]
    pub async fn stop_without_saving(&mut self) -> Result<String> {
        self.stop_and_discard_path().await
    }

    pub async fn stop_and_discard_path(&mut self) -> Result<String> {
        info!("Stopping streaming recorder (discarding audio path)...");

        // Report any dropped audio chunks
        let drops = self.dropped_chunks.load(Ordering::Relaxed);
        if drops > 0 {
            warn!(
                "Recording session: dropped {} audio chunk(s) due to backpressure",
                drops
            );
        }

        // 1. Stop recording (discard WAV path)
        let _ = self.recorder.stop().await?;

        // 2. Wait for worker to finish processing remaining chunks
        if let Some(handle) = self.transcription_handle.take() {
            debug!("Waiting for transcription session task to finish...");
            handle.await.context("Transcription session task failed")?;
        }

        // 3. Drain presentation layer (same as stop() — see comment there).
        if self.event_sink.is_some() {
            let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let snapshot = self.transcript_buffer.lock().await.len();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if self.transcript_buffer.lock().await.len() == snapshot
                    || tokio::time::Instant::now() >= drain_deadline
                {
                    break;
                }
            }
        }
        self.event_sink = None;

        // 4. Return collected transcript
        let transcript = self.transcript_buffer.lock().await.clone();
        Ok(transcript)
    }
}

/// RMS of one captured audio block (linear, 0..~1 for full-scale input).
/// Cheap enough for the CoreAudio callback thread (one pass + one sqrt).
fn block_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    // Accumulate in f64 so a malformed/out-of-range f32 block cannot overflow
    // the sum. Non-finite device samples are treated as silence; NaN/Inf must
    // never cross the typed audio-level transport into Swift.
    let sum_sq = samples.iter().fold(0.0_f64, |sum, sample| {
        let sample = if sample.is_finite() {
            f64::from(*sample)
        } else {
            0.0
        };
        sum + sample * sample
    });
    (sum_sq / samples.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::chunker::{SpeechEvent, SpeechSession, VadGateMode};
    use crate::audio::load_audio_file;
    use crate::pipeline::streaming::transcribe_streaming_samples;
    use crate::stt::whisper;
    use crate::vad;
    use serial_test::serial;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tokio::time::Duration;

    #[test]
    fn block_rms_measures_signal_energy() {
        assert_eq!(block_rms(&[]), 0.0, "empty block must read as silence");
        assert_eq!(block_rms(&[0.0; 512]), 0.0, "digital silence is 0 RMS");
        let full_scale = block_rms(&[1.0, -1.0, 1.0, -1.0]);
        assert!(
            (full_scale - 1.0).abs() < 1e-6,
            "full-scale square wave must read ~1.0, got {full_scale}"
        );
        let half = block_rms(&[0.5, -0.5, 0.5, -0.5]);
        assert!(
            (half - 0.5).abs() < 1e-6,
            "half-scale square wave must read ~0.5, got {half}"
        );
    }

    #[test]
    fn block_rms_orders_quiet_and_loud_finite_levels() {
        let silence = block_rms(&[0.0; 512]);
        let quiet = block_rms(&[0.01, -0.01, 0.01, -0.01]);
        let loud = block_rms(&[0.8, -0.8, 0.8, -0.8]);

        assert!(silence.is_finite() && quiet.is_finite() && loud.is_finite());
        assert!(
            silence < quiet && quiet < loud,
            "expected monotonic energy, got silence={silence}, quiet={quiet}, loud={loud}"
        );
        assert_eq!(
            block_rms(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]),
            0.0,
            "non-finite capture samples must not poison the meter transport"
        );
    }

    /// Delivery probe for the selected real input. During the nine-second run,
    /// keep 0-3s silent, speak quietly during 3-6s, then loudly during 6-9s.
    /// The test is ignored by default because it requires TCC microphone access
    /// and a human-marked acoustic sequence.
    #[tokio::test]
    #[ignore = "requires selected microphone + TCC and silence/quiet/loud operator input"]
    async fn real_input_rms_probe() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping real RMS probe (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let started = std::time::Instant::now();
        let (level_tx, level_rx) = std::sync::mpsc::sync_channel::<(f32, f32)>(1024);
        let mut recorder = Recorder::new().expect("Failed to initialize selected microphone");
        recorder.set_callback(Box::new(move |samples| {
            let _ = level_tx.try_send((started.elapsed().as_secs_f32(), block_rms(samples)));
        }));

        eprintln!("RMS probe: 0-3s SILENCE, 3-6s QUIET SPEECH, 6-9s LOUD SPEECH");
        recorder
            .start()
            .await
            .expect("Failed to start selected microphone");
        tokio::time::sleep(Duration::from_secs(9)).await;
        let audio_path = recorder
            .stop()
            .await
            .expect("Failed to stop selected microphone");
        if let Some(path) = audio_path {
            let _ = std::fs::remove_file(path);
        }

        let mut windows = [Vec::<f32>::new(), Vec::<f32>::new(), Vec::<f32>::new()];
        for (elapsed, rms) in level_rx.try_iter() {
            assert!(rms.is_finite(), "real input emitted non-finite RMS: {rms}");
            let index = (elapsed / 3.0).floor() as usize;
            if let Some(window) = windows.get_mut(index) {
                window.push(rms);
            }
        }

        let means = windows.map(|window| {
            assert!(
                !window.is_empty(),
                "real input probe window captured no blocks"
            );
            window.iter().copied().sum::<f32>() / window.len() as f32
        });
        eprintln!(
            "RMS probe means: silence={:.6}, quiet={:.6}, loud={:.6}",
            means[0], means[1], means[2]
        );
        assert!(
            means[0] < means[1] && means[1] < means[2],
            "selected input did not produce ordered silence/quiet/loud energy: {means:?}"
        );
    }

    /// Five-minute delivery probe for capture/backpressure stability. This runs
    /// the production `StreamingRecorder` path against the selected input and
    /// reports callback, engine-event, and dropped-chunk counters. It is opt-in
    /// because it needs TCC microphone access and intentionally holds the real
    /// audio device for the full acceptance interval.
    #[tokio::test]
    #[ignore = "requires selected microphone + TCC and a five-minute foreground run"]
    async fn sustained_real_input_pressure_probe() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping sustained mic probe (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let duration_sec = env_f32("CODESCRIBE_E2E_SUSTAIN_SEC", 300.0).max(300.0);
        let level_blocks = Arc::new(AtomicU64::new(0));
        let non_finite_levels = Arc::new(AtomicU64::new(0));
        let level_blocks_for_callback = Arc::clone(&level_blocks);
        let non_finite_for_callback = Arc::clone(&non_finite_levels);
        let sink = Arc::new(crate::pipeline::sinks::CollectorEventSink::new());
        let mut recorder = StreamingRecorder::new().expect("Failed to initialize selected input");
        recorder.set_level_callback(Some(Arc::new(move |rms| {
            level_blocks_for_callback.fetch_add(1, Ordering::Relaxed);
            if !rms.is_finite() {
                non_finite_for_callback.fetch_add(1, Ordering::Relaxed);
            }
        })));
        recorder.set_event_sink(Some(sink.clone()));

        eprintln!("Sustained mic probe: recording selected input for {duration_sec:.0}s");
        recorder
            .start_event_session(None)
            .await
            .expect("Failed to start streaming recorder");
        tokio::time::sleep(Duration::from_secs_f32(duration_sec)).await;
        let (_transcript, audio_path) = recorder
            .stop()
            .await
            .expect("Failed to stop streaming recorder");
        if let Some(path) = audio_path {
            let _ = std::fs::remove_file(path);
        }

        let levels = level_blocks.load(Ordering::Relaxed);
        let invalid = non_finite_levels.load(Ordering::Relaxed);
        let drops = recorder.dropped_chunks.load(Ordering::Relaxed);
        let events = sink.events().len();
        eprintln!(
            "Sustained mic counters: level_blocks={levels}, non_finite={invalid}, dropped_chunks={drops}, engine_events={events}"
        );
        assert!(levels > 0, "selected input produced no capture callbacks");
        assert_eq!(invalid, 0, "real input emitted non-finite RMS levels");
        assert_eq!(drops, 0, "sustained recording dropped audio chunks");
    }

    #[tokio::test]
    async fn start_event_session_requires_event_sink() {
        let mut recorder = StreamingRecorder::new().expect("Failed to create recorder");
        let err = recorder
            .start_event_session(Some("en".to_string()))
            .await
            .expect_err("start_event_session should fail when event sink is missing");
        assert!(
            err.to_string().contains("requires event_sink"),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    #[ignore] // Manual: requires microphone + Silero model (set CODESCRIBE_E2E_MIC=1)
    fn test_vad_gate_live_chunk_sizes() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping mic gate test (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let model_path = vad::default_model_path();
        if !model_path.exists() {
            eprintln!(
                "Skipping: Silero VAD model not found at {}",
                model_path.display()
            );
            return;
        }

        let record_sec = env_f32("CODESCRIBE_E2E_MIC_SEC", 6.0).max(2.0);
        println!("Speak now for ~{:.1}s...", record_sec);

        let mut recorder = Recorder::new().expect("Failed to create recorder");
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let wav_path = rt
            .block_on(async {
                recorder.start().await.expect("Failed to start recorder");
                tokio::time::sleep(Duration::from_secs_f32(record_sec)).await;
                recorder.stop().await.expect("Failed to stop recorder")
            })
            .expect("No WAV produced");

        let (samples, sample_rate) =
            load_audio_file(&wav_path).expect("Failed to load recorded audio");

        let mut resampler = vad::Resampler::new(sample_rate);
        let samples_16k = resampler.resample(&samples);
        let chunk_sec = 4.0f32;
        let chunk_limit = (vad::VAD_SAMPLE_RATE as f32 * chunk_sec) as usize;

        let cases = [
            ("lt", chunk_limit / 2),
            ("eq", chunk_limit),
            ("gt", chunk_limit * 2),
        ];

        for (label, block_len) in cases {
            let mut session = SpeechSession::new_stream(vad::VAD_SAMPLE_RATE, chunk_sec, 0.0);
            let mut chunk_events = 0usize;
            let mut idx = 0usize;
            while idx < samples_16k.len() {
                let end = (idx + block_len).min(samples_16k.len());
                let slice = &samples_16k[idx..end];
                for event in session.feed(slice, vad::VAD_SAMPLE_RATE) {
                    if matches!(event, SpeechEvent::Chunk(_)) {
                        chunk_events += 1;
                    }
                }
                idx = end;
            }
            if let Some(SpeechEvent::Chunk(_)) = session.flush() {
                chunk_events += 1;
            }

            assert!(
                chunk_events > 0,
                "Expected at least one chunk for case {} (block_len={})",
                label,
                block_len
            );
        }

        let _ = fs::remove_file(&wav_path);
    }

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
            let mut postprocessor = crate::pipeline::stream_postprocess::StreamPostProcessor::new();
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

    fn is_terminal_no_speech_artifact(file_name: &str) -> bool {
        file_name.contains("no-speech") || file_name.ends_with("_failed.wav")
    }

    #[test]
    fn terminal_no_speech_artifact_filter_is_name_bounded() {
        assert!(is_terminal_no_speech_artifact(
            "20260709_120000_no-speech_raw.wav"
        ));
        assert!(is_terminal_no_speech_artifact(
            "20260709_120001_dictation_failed.wav"
        ));
        assert!(!is_terminal_no_speech_artifact(
            "20260709_120002_failed-but-recovered_raw.wav"
        ));
        assert!(!is_terminal_no_speech_artifact(
            "03_algorytm-ma-zlozonosc.wav"
        ));
        assert!(!is_terminal_no_speech_artifact("dictation_failed.m4a"));
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

    fn vad_index_drift_tolerance(input_sr: u32) -> usize {
        ((vad::CHUNK_SIZE as f32 * input_sr as f32) / vad::VAD_SAMPLE_RATE as f32) as usize
    }

    #[test]
    #[serial]
    fn test_vad_index_sync_no_drift() {
        let input_sr = 48000u32;
        let callback_size = 1024usize;
        let num_callbacks = 100usize;

        let mut session = SpeechSession::new_stream(input_sr, 15.0, 0.0);
        assert_eq!(
            session.gate_mode(),
            crate::audio::chunker::VadGateMode::Supervisor,
            "drift guard must explicitly validate Supervisor mode"
        );

        let freq = 440.0f32;
        let mut phase = 0.0f32;
        let phase_inc = 2.0 * std::f32::consts::PI * freq / input_sr as f32;

        for _ in 0..num_callbacks {
            let mut buf = Vec::with_capacity(callback_size);
            for _ in 0..callback_size {
                buf.push(phase.sin() * 0.5);
                phase += phase_inc;
            }
            let _ = session.feed(&buf, input_sr);
        }

        let total_raw = num_callbacks * callback_size;
        assert_eq!(
            session.raw_cursor(),
            total_raw,
            "raw_cursor should equal total input samples"
        );

        let vad_sample = session
            .vad_current_sample()
            .expect("Supervisor mode should expose VAD sample index");
        let mapped = session.vad_to_raw_index_pub(vad_sample);
        let raw_cur = session.raw_cursor();
        let drift = mapped.abs_diff(raw_cur);
        let tolerance = vad_index_drift_tolerance(input_sr);
        assert!(
            drift <= tolerance,
            "VAD index drift too large: mapped={} raw_cursor={} drift={} tolerance={}",
            mapped,
            raw_cur,
            drift,
            tolerance
        );

        assert!(
            session.vad_resample_buf_len() < vad::CHUNK_SIZE,
            "Residual buffer should be < CHUNK_SIZE, got {}",
            session.vad_resample_buf_len()
        );
    }

    #[test]
    #[serial]
    fn test_supervisor_busy_flush_keeps_boundary_and_speech_accounting() {
        let input_sr = 48000u32;
        let callback_size = 1024usize;
        let num_callbacks = 210usize;

        let mut session = SpeechSession::new_utterance_with_silence(input_sr, 10.0);
        assert_eq!(
            session.gate_mode(),
            VadGateMode::Supervisor,
            "busy flush guard must explicitly validate Supervisor mode"
        );

        // Deterministic open segment even when VAD model is unavailable.
        session.set_vad_threshold_for_test(-1.0);

        let mut interim_events = 0usize;
        let mut accounted_speech_vad_samples = 0u64;

        for _ in 0..num_callbacks {
            let buf = vec![0.0f32; callback_size];
            for event in session.feed(&buf, input_sr) {
                let event_speech = session.take_event_speech_vad_samples();
                accounted_speech_vad_samples =
                    accounted_speech_vad_samples.saturating_add(event_speech);
                match event {
                    SpeechEvent::Utterance(samples) => {
                        interim_events = interim_events.saturating_add(1);
                        assert!(
                            !samples.is_empty(),
                            "busy interim event should never carry empty audio"
                        );
                        assert!(
                            event_speech > 0,
                            "busy interim event should carry positive speech sample accounting"
                        );
                    }
                    SpeechEvent::UtteranceFinal(_) => {
                        panic!("unexpected UtteranceFinal before flush in long-silence test")
                    }
                    SpeechEvent::Chunk(_) => {
                        panic!("unexpected Chunk event in utterance mode")
                    }
                }
            }
        }

        assert!(
            interim_events > 0,
            "busy callback run should emit at least one interim utterance before flush"
        );

        let flush = session.flush();
        let flush_speech = session.take_event_speech_vad_samples();
        accounted_speech_vad_samples = accounted_speech_vad_samples.saturating_add(flush_speech);

        let flush_len = match flush {
            Some(SpeechEvent::UtteranceFinal(samples)) => samples.len(),
            Some(SpeechEvent::Utterance(_)) => {
                panic!("flush should emit final utterance event")
            }
            Some(SpeechEvent::Chunk(_)) => {
                panic!("flush should not emit stream chunk in utterance mode")
            }
            None => panic!("flush should preserve active Supervisor boundary under busy load"),
        };
        assert!(flush_len > 0, "flush final event should include audio");
        assert!(
            flush_speech > 0,
            "flush final event should carry pending speech sample accounting"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            0,
            "speech accounting queue should be empty after consuming flush event"
        );

        let total_raw = num_callbacks * callback_size;
        assert_eq!(
            session.raw_cursor(),
            total_raw,
            "raw cursor should stay aligned with callback sample count under busy load"
        );

        let vad_sample = session
            .vad_current_sample()
            .expect("Supervisor mode should expose VAD sample index");
        let mapped = session.vad_to_raw_index_pub(vad_sample);
        let raw_cur = session.raw_cursor();
        let drift = mapped.abs_diff(raw_cur);
        let tolerance = vad_index_drift_tolerance(input_sr);
        assert!(
            drift <= tolerance,
            "busy path drift too large: mapped={} raw_cursor={} drift={} tolerance={}",
            mapped,
            raw_cur,
            drift,
            tolerance
        );
        assert_eq!(
            accounted_speech_vad_samples as usize, vad_sample,
            "sum of emitted speech sample accounting should equal processed VAD samples"
        );
    }

    /// Run VAD on real WAV files and report segmentation quality.
    #[test]
    fn test_vad_supervisor_segments_real_audio() {
        let corpus_dir =
            std::path::PathBuf::from(shellexpand::tilde("~/.codescribe/transcriptions").as_ref());
        if !corpus_dir.exists() {
            eprintln!("Skipping: no transcriptions dir");
            return;
        }
        let model_path = vad::default_model_path();
        if !model_path.exists() {
            eprintln!("Skipping: no Silero model");
            return;
        }

        let edge_cases = [
            "192322_nie-zmienia-to_raw.wav",
            "133135_no-dobra-teraz_raw.wav",
            "182340_klaudiusz-zacznijmy-od_raw.wav",
            "001615_dziekuje---dziekuje_raw.wav",
            "184818_dzien-dobry-chcialem_raw.wav",
        ];

        let mut wavs: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(dirs) = fs::read_dir(&corpus_dir) {
            for dir_entry in dirs.flatten() {
                if !dir_entry.path().is_dir() {
                    continue;
                }
                for case in &edge_cases {
                    let candidate = dir_entry.path().join(case);
                    if candidate.exists() {
                        wavs.push(candidate);
                    }
                }
            }
        }
        if wavs.is_empty() {
            let mut dirs: Vec<_> = fs::read_dir(&corpus_dir)
                .unwrap()
                .flatten()
                .filter(|e| e.path().is_dir())
                .collect();
            dirs.sort_by_key(|e| e.file_name());
            dirs.reverse();
            for dir in dirs.iter().take(2) {
                if let Ok(entries) = fs::read_dir(dir.path()) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|s| s.to_str()) == Some("wav") {
                            let fname = p.file_name().unwrap_or_default().to_string_lossy();
                            // Terminal failed/no-speech artifacts should not be scored as VAD segmentation misses.
                            if is_terminal_no_speech_artifact(&fname) {
                                continue;
                            }
                            wavs.push(p);
                            if wavs.len() >= 5 {
                                break;
                            }
                        }
                    }
                }
            }
        }

        println!("\n╭─── VAD v5 Segmentation Test ───────────────────────╮");
        let mut all_pass = true;

        for wav_path in &wavs {
            let fname = wav_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let (samples, sample_rate) = match load_audio_file(wav_path) {
                Ok(v) => v,
                Err(e) => {
                    println!("│ SKIP {} — {}", fname, e);
                    continue;
                }
            };
            let audio_sec = samples.len() as f32 / sample_rate as f32;

            let vad_config = vad::VadConfig {
                threshold: 0.50,
                min_speech_duration_sec: 0.05,
                max_silence_duration_sec: 0.20,
                max_utterance_sec: 300.0,
                pre_roll_sec: 0.064,
                ..vad::VadConfig::default()
            };
            let mut silero = vad::SileroVad::new(&model_path, vad_config).expect("load Silero");
            let mut resampler = vad::Resampler::new(sample_rate);
            let samples_16k = resampler.resample(&samples);

            let mut above = 0usize;
            let mut total = 0usize;
            for chunk in samples_16k.chunks(vad::CHUNK_SIZE) {
                if chunk.len() < vad::CHUNK_SIZE {
                    break;
                }
                total += 1;
                if silero.predict(chunk).unwrap_or(0.0) >= 0.5 {
                    above += 1;
                }
            }

            let callback_size = 1024usize;
            let mut session = SpeechSession::new_utterance(sample_rate);
            let mut events = Vec::new();
            let mut offset = 0usize;
            while offset < samples.len() {
                let end = (offset + callback_size).min(samples.len());
                for event in session.feed(&samples[offset..end], sample_rate) {
                    events.push(event);
                }
                offset = end;
            }
            if let Some(event) = session.flush() {
                events.push(event);
            }

            let n_segments = events.len();
            let speech_samples: usize = events
                .iter()
                .map(|e| match e {
                    SpeechEvent::Utterance(s)
                    | SpeechEvent::UtteranceFinal(s)
                    | SpeechEvent::Chunk(s) => s.len(),
                })
                .sum();
            let speech_sec = speech_samples as f32 / sample_rate as f32;
            let silence_cut = audio_sec - speech_sec;
            let cut_pct = if audio_sec > 0.0 {
                silence_cut / audio_sec * 100.0
            } else {
                0.0
            };

            let raw_txt = wav_path.to_string_lossy().replace("_raw.wav", "_raw.txt");
            let old_len = fs::read_to_string(&raw_txt).map(|s| s.len()).unwrap_or(0);

            println!("│");
            println!("│ 📁 {}", fname);
            println!(
                "│    Audio: {:.1}s | VAD speech: {:.0}% ({}/{} frames)",
                audio_sec,
                if total > 0 {
                    above as f32 / total as f32 * 100.0
                } else {
                    0.0
                },
                above,
                total,
            );
            println!(
                "│    Segments: {} | Speech: {:.1}s | Silence cut: {:.1}s ({:.0}%)",
                n_segments, speech_sec, silence_cut, cut_pct,
            );
            println!("│    Old transcript: {} chars", old_len,);

            let old_text = fs::read_to_string(&raw_txt).unwrap_or_default();
            let halluc_count = old_text.matches("Thank you").count()
                + old_text.matches("Dziękuję.").count()
                + old_text.matches(".com/").count();
            if halluc_count > 2 {
                println!(
                    "│    ⚠ Old transcript had {} hallucination markers (Thank you/Dziękuję./.com/)",
                    halluc_count,
                );
                println!(
                    "│    ✅ VAD v5 would cut {:.1}s silence → these tails eliminated",
                    silence_cut,
                );
            }

            if above == 0 && audio_sec > 1.0 {
                println!("│    ❌ VAD detected NO speech — possible model issue");
                all_pass = false;
            }
        }

        println!("│");
        println!("╰────────────────────────────────────────────────────╯\n");

        assert!(all_pass, "Some files had zero speech detection");
    }

    /// Test the categorical silence gate on data_assets WAV files.
    ///
    /// Simulates the live recording pipeline: feeds audio through SpeechSession
    /// in Supervisor/Utterance mode, collects events with their speech_vad_samples,
    /// and checks which chunks the silence gate would drop.
    #[test]
    #[serial]
    fn test_silence_gate_on_data_assets() {
        use crate::pipeline::streaming::should_drop_silence_chunk;

        let data_dir =
            std::path::PathBuf::from(shellexpand::tilde("~/.codescribe/data_assets").as_ref());
        if !data_dir.exists() {
            eprintln!("Skipping: no data_assets dir");
            return;
        }

        let wavs: Vec<_> = [
            "01_no-to-dobra.wav",
            "02_kubernetes-wymaga-konfiguracji.wav",
            "03_algorytm-ma-zlozonosc.wav",
            "04_runda-3-czyli.wav",
        ]
        .iter()
        .map(|f| data_dir.join(f))
        .filter(|p| p.exists())
        .collect();

        if wavs.is_empty() {
            eprintln!("Skipping: no WAV files found in data_assets");
            return;
        }

        println!("\n╭─── Silence Gate Test (data_assets) ────────────────╮");

        for wav_path in &wavs {
            let fname = wav_path.file_name().unwrap_or_default().to_string_lossy();
            let (samples, sample_rate) = match load_audio_file(wav_path) {
                Ok(v) => v,
                Err(e) => {
                    println!("│ SKIP {} — {}", fname, e);
                    continue;
                }
            };
            let audio_sec = samples.len() as f32 / sample_rate as f32;

            // Simulate live recording: feed audio in ~1024-sample callbacks
            let callback_size = 1024usize;
            let mut session = SpeechSession::new_utterance(sample_rate);
            assert_eq!(session.gate_mode(), VadGateMode::Supervisor);

            let mut events_with_speech = Vec::new();
            let mut offset = 0usize;
            while offset < samples.len() {
                let end = (offset + callback_size).min(samples.len());
                for event in session.feed(&samples[offset..end], sample_rate) {
                    let speech_vad = session.take_event_speech_vad_samples();
                    events_with_speech.push((event, speech_vad));
                }
                offset = end;
            }
            if let Some(event) = session.flush() {
                let speech_vad = session.take_event_speech_vad_samples();
                events_with_speech.push((event, speech_vad));
            }

            let mut dropped = 0usize;
            let mut kept = 0usize;
            let mut dropped_sec = 0.0f32;

            println!("│");
            println!("│ {} ({:.1}s)", fname, audio_sec);

            for (i, (event, speech_vad_samples)) in events_with_speech.iter().enumerate() {
                let chunk_len = match event {
                    SpeechEvent::Utterance(s)
                    | SpeechEvent::UtteranceFinal(s)
                    | SpeechEvent::Chunk(s) => s.len(),
                };
                let is_final = matches!(event, SpeechEvent::UtteranceFinal(_));
                let chunk_sec = chunk_len as f32 / sample_rate as f32;
                let audio_16k = (chunk_len as f64 * f64::from(vad::VAD_SAMPLE_RATE)
                    / f64::from(sample_rate)) as u64;
                let ratio = if audio_16k > 0 {
                    *speech_vad_samples as f32 / audio_16k as f32
                } else {
                    0.0
                };

                let would_drop = should_drop_silence_chunk(
                    chunk_len,
                    sample_rate,
                    *speech_vad_samples,
                    is_final,
                );

                let tag = if would_drop { "DROP" } else { "KEEP" };
                let kind = if is_final { "Final" } else { "Interim" };
                println!(
                    "│   [{:2}] {} {}: {:.2}s speech_ratio={:.0}% (vad_samples={})",
                    i,
                    tag,
                    kind,
                    chunk_sec,
                    ratio * 100.0,
                    speech_vad_samples,
                );

                if would_drop {
                    dropped += 1;
                    dropped_sec += chunk_sec;
                } else {
                    kept += 1;
                }
            }

            println!(
                "│   → kept={} dropped={} (saved {:.1}s of Whisper inference on silence)",
                kept, dropped, dropped_sec,
            );
        }

        println!("│");
        println!("╰────────────────────────────────────────────────────╯\n");
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
