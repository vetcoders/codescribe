//! Manual replay benchmark: 4-second PCM observations with 1-second overlap.
//! Uses the product loader and single-window decoder, without delivery or APIs.

use anyhow::{Context, Result, ensure};
use codescribe_core::{audio::load_audio_file, stt::whisper::LocalWhisperEngine};
use serde_json::json;
use std::{env, path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    ensure!(
        args.len() >= 3,
        "usage: whisper_decode_bench MODEL_DIR AUDIO [ROUNDS] [MAX_WINDOWS]"
    );
    let model = PathBuf::from(&args[1]).canonicalize()?;
    let audio = PathBuf::from(&args[2]).canonicalize()?;
    let rounds: usize = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(2);
    let max_windows: usize = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(0);
    ensure!(rounds > 0, "rounds must be positive");
    let (samples, rate) = load_audio_file(&audio).context("load replay PCM")?;
    ensure!(!samples.is_empty(), "empty audio");
    let window = rate as usize * 4;
    let stride = rate as usize * 3;
    let started = Instant::now();
    let mut engine = LocalWhisperEngine::new(&model)?;
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    engine.transcribe_with_language_segments(
        &samples[..samples.len().min(window)],
        rate,
        Some("pl"),
    )?;
    let warmup_ms = started.elapsed().as_secs_f64() * 1000.0;
    let all_starts: Vec<usize> = (0..samples.len()).step_by(stride).collect();
    let starts = if max_windows > 1 && all_starts.len() > max_windows {
        (0..max_windows)
            .map(|i| all_starts[i * (all_starts.len() - 1) / (max_windows - 1)])
            .collect()
    } else if max_windows == 1 {
        vec![0]
    } else {
        all_starts
    };
    let mut observations = Vec::new();
    for round in 0..rounds {
        for &start in &starts {
            let end = (start + window).min(samples.len());
            let started = Instant::now();
            let result =
                engine.transcribe_with_language_segments(&samples[start..end], rate, Some("pl"))?;
            observations.push(json!({
                "round": round, "start_sample": start, "end_sample": end,
                "decode_ms": started.elapsed().as_secs_f64() * 1000.0,
                "transcript": result,
            }));
        }
    }
    // Explicit manual file replay supplies one transcript for comparison with
    // the human reference. It does not introduce an automatic full-file pass.
    let started = Instant::now();
    let full_transcript =
        engine.transcribe_long_with_language_segments(&samples, rate, Some("pl"))?;
    let full_decode_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "source_commit": option_env!("GIT_COMMIT_HASH"),
            "model_path": model, "audio_path": audio, "sample_rate": rate,
            "window_s": 4, "overlap_s": 1, "rounds": rounds,
            "model_load_ms": load_ms, "warmup_ms": warmup_ms,
            "observations": observations,
            "full_decode_ms": full_decode_ms, "full_transcript": full_transcript,
        }))?
    );
    Ok(())
}
