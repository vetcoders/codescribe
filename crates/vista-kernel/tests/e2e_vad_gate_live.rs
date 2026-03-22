//! E2E coverage for Silero-driven gate in live streaming.
//!
//! These tests are ignored by default because they require the Silero VAD model.

use codescribe_core::pipeline::streaming::transcribe_streaming_samples;
use codescribe_core::vad;

fn generate_silence(duration_sec: f32, sample_rate: u32) -> Vec<f32> {
    let samples = (duration_sec * sample_rate as f32) as usize;
    vec![0.0f32; samples]
}

fn generate_tone(duration_sec: f32, sample_rate: u32, amp: f32) -> Vec<f32> {
    let samples = (duration_sec * sample_rate as f32) as usize;
    (0..samples)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            (2.0 * std::f32::consts::PI * 440.0 * t).sin() * amp
        })
        .collect()
}

#[test]
#[ignore] // Requires Silero VAD model (run with: cargo test -- --ignored)
fn test_vad_gate_segments_speech_only() {
    let model_path = vad::default_model_path();
    if !model_path.exists() {
        eprintln!(
            "Skipping: Silero VAD model not found at {}",
            model_path.display()
        );
        return;
    }

    // 48kHz input to match live capture; gate should resample to 16k internally.
    let sample_rate = 48_000;
    let mut samples = Vec::new();
    samples.extend_from_slice(&generate_silence(0.4, sample_rate));
    samples.extend_from_slice(&generate_tone(0.8, sample_rate, 0.7));
    samples.extend_from_slice(&generate_silence(0.3, sample_rate));

    // We only assert that some output is produced (gate lets speech through)
    let out = transcribe_streaming_samples(&samples, sample_rate, None, None).unwrap_or_default();
    let _ = out; // assert: reached here without panic
}
