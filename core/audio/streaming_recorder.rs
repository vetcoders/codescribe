use crate::audio::recorder::{Recorder, RecorderConfig};
use crate::pipeline::contracts::DeltaSink;
use crate::pipeline::stream_postprocess::StreamPostProcessor;
use crate::pipeline::streaming::{
    buffered_transcription_worker, env_bool_default, stream_log_path, transcription_worker,
};
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info};

// Re-export public API that was moved to pipeline::streaming
pub use crate::pipeline::streaming::transcribe_streaming_samples;

pub struct StreamingRecorder {
    pub recorder: Recorder,
    transcript_buffer: Arc<Mutex<String>>,
    transcription_handle: Option<JoinHandle<()>>,
    sample_rate: u32,
    delta_callback: Option<Arc<dyn DeltaSink>>,
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

    pub fn set_delta_callback(&mut self, callback: Option<Arc<dyn DeltaSink>>) {
        self.delta_callback = callback;
    }

    pub async fn start(&mut self, language: Option<String>) -> Result<()> {
        let use_buffered_stream = env_bool_default("CODESCRIBE_BUFFERED_STREAM", true);
        self.start_with_buffered(language, use_buffered_stream)
            .await
    }

    pub async fn start_with_buffered(
        &mut self,
        language: Option<String>,
        use_buffered_stream: bool,
    ) -> Result<()> {
        // Clear previous transcript
        *self.transcript_buffer.lock().await = String::new();

        // Create channel for audio chunks
        let (tx, rx) = mpsc::channel::<Vec<f32>>(500);

        // Setup callback to send audio data
        self.recorder.set_callback(Box::new(move |data| {
            if let Err(_e) = tx.try_send(data.to_vec()) {
                // If channel is full, we drop audio (better than blocking)
            }
        }));

        // Start the actual audio stream first, so we know the *real* sample rate (often 48kHz).
        self.recorder.start().await?;

        // Update sample rate to the one used by the input stream.
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
        let log_path = stream_log_path();
        let delta_callback = self.delta_callback.clone();
        self.transcription_handle = Some(tokio::spawn(async move {
            if use_buffered_stream {
                buffered_transcription_worker(
                    rx,
                    transcript_buffer,
                    actual_sample_rate,
                    language,
                    delta_callback,
                    None, // vad_stop_callback — not used in app context
                    log_path,
                )
                .await;
            } else {
                // Always-on contract: every transcript passes through postprocessor
                // (lexicon + cleanup + semantic gate) regardless of buffered mode.
                let postprocessor = Some(StreamPostProcessor::new());
                transcription_worker(
                    rx,
                    transcript_buffer,
                    actual_sample_rate,
                    language,
                    postprocessor,
                    delta_callback,
                    log_path,
                )
                .await;
            }
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

    pub async fn stop_without_saving(&mut self) -> Result<String> {
        info!("Stopping streaming recorder (no WAV)...");

        // 1. Stop recording (discard WAV path)
        let _ = self.recorder.stop().await?;

        // 2. Wait for worker to finish processing remaining chunks
        if let Some(handle) = self.transcription_handle.take() {
            debug!("Waiting for transcription worker to finish...");
            handle.await.context("Transcription worker failed")?;
        }

        // 3. Return collected transcript
        let transcript = self.transcript_buffer.lock().await.clone();
        Ok(transcript)
    }
}

// Note: calculate_rms_db removed - now using vad::speech_probability for voice detection

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::chunker::{SpeechEvent, SpeechSession};
    use crate::audio::load_audio_file;
    use crate::stt::whisper;
    use crate::vad;
    use serial_test::serial;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tokio::time::Duration;

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

    #[test]
    fn test_vad_index_sync_no_drift() {
        let input_sr = 48000u32;
        let callback_size = 1024usize;
        let num_callbacks = 100usize;

        let mut session = SpeechSession::new_stream(input_sr, 15.0, 0.0);

        if session.gate_mode() != crate::audio::chunker::VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

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

        if let Some(vad_sample) = session.vad_current_sample() {
            let mapped = session.vad_to_raw_index_pub(vad_sample);
            let raw_cur = session.raw_cursor();
            let drift = mapped.abs_diff(raw_cur);
            let vad_chunk = 512usize;
            let vad_sr = 16000.0f32;
            let tolerance = ((vad_chunk as f32 * input_sr as f32) / vad_sr) as usize;
            assert!(
                drift <= tolerance,
                "VAD index drift too large: mapped={} raw_cursor={} drift={} tolerance={}",
                mapped,
                raw_cur,
                drift,
                tolerance
            );
        }

        let vad_chunk_size = 512usize;
        assert!(
            session.vad_resample_buf_len() < vad_chunk_size,
            "Residual buffer should be < CHUNK_SIZE, got {}",
            session.vad_resample_buf_len()
        );
    }

    /// Run VAD v5 on real WAV files and report segmentation quality.
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
                    SpeechEvent::Utterance(s) | SpeechEvent::Chunk(s) => s.len(),
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
