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

/// Time each fixture through the production stop-path and print the stage
/// breakdown.
///
/// `engine_overhead_ms` is what the total leaves over once lock wait, model
/// load and decode are subtracted — the part no instrumented stage claims, and
/// therefore the one worth watching. The default fixture sequence repeats the
/// first clip so the cold and warm costs of the same audio sit side by side.
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

    // A1 TTL proof window: with BASELINE_IDLE_PROBE_SECS set, stay idle past
    // the reaper TTL and print RSS every 30 s so the weight-drop shows up as a
    // real resident-memory delta (not just a dropped reference).
    if let Some(probe_secs) = std::env::var("BASELINE_IDLE_PROBE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        let started = Instant::now();
        println!("idle_probe_start rss_mb={}", rss_mb());
        while started.elapsed().as_secs() < probe_secs {
            std::thread::sleep(std::time::Duration::from_secs(30));
            println!(
                "idle_probe t={}s rss_mb={}",
                started.elapsed().as_secs(),
                rss_mb()
            );
        }
    }
    Ok(())
}

/// Resident set size of this process in MiB via `ps` (portable enough for a
/// macOS-only harness; phys_footprint needs entitlements `ps` does not).
fn rss_mb() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}
