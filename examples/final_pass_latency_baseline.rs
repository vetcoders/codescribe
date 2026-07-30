//! A2 final-pass latency baseline: per-stage timing on fixture WAVs.
//!
//! Exercises the exact production stop-path call
//! (`codescribe_core::stt::transcribe_file_verdict` through the Whisper
//! singleton) and reads the thread-local stage timing the instrumentation
//! records: engine lock wait, cold model load, pure decode span.
//!
//! Usage:
//!   CODESCRIBE_STT_ENGINE=candle cargo run --release --example final_pass_latency_baseline [wav ...]
//!
//! Default fixture sequence (cold → warm → long) when no args are given:
//!   1. tests/assets/data_assets/01_no-to-dobra.wav      (cold: pays model load)
//!   2. tests/assets/data_assets/01_no-to-dobra.wav      (warm: same clip)
//!   3. tests/assets/data_assets/05_apple-live-parity.wav (long dictation, warm)

use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        paths = vec![
            PathBuf::from("tests/assets/data_assets/01_no-to-dobra.wav"),
            PathBuf::from("tests/assets/data_assets/01_no-to-dobra.wav"),
            PathBuf::from("tests/assets/data_assets/05_apple-live-parity.wav"),
        ];
    }

    println!(
        "run | file | total_ms | queue_lock_ms | model_load_ms | cold_load | inference_ms | engine_overhead_ms | chars"
    );
    for (index, path) in paths.iter().enumerate() {
        anyhow::ensure!(path.exists(), "fixture not found: {}", path.display());
        let started = Instant::now();
        let verdict = codescribe_core::stt::transcribe_file_verdict(path, Some("pl"))?;
        let total_ms = started.elapsed().as_millis() as u64;
        let timing = codescribe_core::stt::whisper::take_final_pass_timing();
        let overhead_ms = total_ms
            .saturating_sub(timing.lock_wait_ms + timing.model_load_ms + timing.inference_ms);
        println!(
            "{} | {} | {} | {} | {} | {} | {} | {} | {}",
            index + 1,
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            total_ms,
            timing.lock_wait_ms,
            timing.model_load_ms,
            timing.cold_load,
            timing.inference_ms,
            overhead_ms,
            verdict.text.len(),
        );
    }
    Ok(())
}
