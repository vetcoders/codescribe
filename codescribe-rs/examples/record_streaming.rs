//! Example demonstrating Recorder with live snapshots (for streaming STT)
//!
//! Run with: cargo run --example record_streaming

use codescribe::{Recorder, RecorderConfig};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .compact()
        .init();

    println!("\n=== CodeScribe Recorder - Streaming Mode Test ===\n");

    // Create recorder with auto-silence disabled (manual control)
    let mut config = RecorderConfig::default();
    config.auto_silence = false; // Don't auto-stop on silence

    let mut recorder = Recorder::with_config(config)?;
    println!("✓ Recorder created (auto-silence disabled)");

    // Start recording
    recorder.start().await?;
    println!("✓ Recording started - speak continuously!");
    println!("  Taking snapshots every second for 5 seconds...\n");

    // Take snapshots every second
    for i in 1..=5 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        
        // Try to get snapshot (min 0.5 seconds of audio)
        if let Some(path) = recorder.snapshot_wav(0.5)? {
            println!("[Snapshot {}] Saved to: {:?}", i, path);
            let diag = recorder.diagnostics();
            println!("            Frames: {}, Duration: {:.2}s", 
                     diag.snapshot_frames, 
                     diag.snapshot_frames as f32 / 16000.0);
        } else {
            println!("[Snapshot {}] Not enough audio yet", i);
        }
    }

    // Stop recording
    println!("\nStopping recording...");
    if let Some(path) = recorder.stop().await? {
        println!("✓ Final recording saved to: {:?}", path);
        println!("  Total duration: {:.2}s", recorder.last_duration());
    }

    println!("\n=== Test Complete ===\n");
    Ok(())
}
