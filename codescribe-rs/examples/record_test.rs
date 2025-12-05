//! Simple example demonstrating Recorder usage
//!
//! Run with: cargo run --example record_test

use codescribe::Recorder;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging to see what's happening
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .compact()
        .init();

    println!("\n=== CodeScribe Recorder Test ===\n");

    // Create recorder with default config (16kHz mono, -45dB silence threshold)
    let mut recorder = Recorder::new()?;
    println!("✓ Recorder created successfully");

    // Start recording
    recorder.start().await?;
    println!("✓ Recording started - speak now!");
    println!("  (Recording will stop after 3 seconds or when silence is detected)\n");

    // Record for 3 seconds (or until silence detected)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Stop and save to WAV file
    println!("Stopping recording...");
    if let Some(path) = recorder.stop().await? {
        println!("\n✓ Recording saved!");
        println!("  Path: {:?}", path);
        println!("  Duration: {:.2}s", recorder.last_duration());
        
        let diagnostics = recorder.diagnostics();
        println!("  Frames: {}", diagnostics.frames);
        println!("  Bytes: {}", diagnostics.bytes);
    } else {
        println!("\n⚠ No audio captured (buffer was empty)");
    }

    println!("\n=== Test Complete ===\n");
    Ok(())
}
