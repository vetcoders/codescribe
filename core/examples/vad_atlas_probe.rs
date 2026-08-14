//! Evidence probe: the PRODUCTION Silero VAD spectrum over a take WAV.
//!
//! Same embedded `silero_vad.onnx`, same `VadConfig::default()`, same
//! resampler the engine uses — fed in canonical 512-sample (32 ms @ 16 kHz)
//! chunks. Emits one JSON with per-chunk speech probability plus the
//! waveform envelope (RMS / peak) on the same chunk grid, so word ranges
//! from a seal-atlas dump can be overlaid on the identical time axis.
//!
//! Usage:
//!   cargo run -p codescribe-core --example vad_atlas_probe -- <take.wav> <out.json>

use codescribe_core::audio::{load_audio_file, resample_to_16k};
use codescribe_core::vad::{AccumulatingVad, CHUNK_SIZE, VAD_SAMPLE_RATE};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let wav = args
        .next()
        .expect("usage: vad_atlas_probe <take.wav> <out.json>");
    let out = args
        .next()
        .expect("usage: vad_atlas_probe <take.wav> <out.json>");

    let (samples, capture_rate) = load_audio_file(std::path::Path::new(&wav))?;
    let capture_len = samples.len() as u64;
    let mono16k = resample_to_16k(&samples, capture_rate);

    // 16 kHz input → AccumulatingVad never resamples again; each feed of one
    // full chunk runs exactly one Silero inference, so probs[i] belongs to
    // samples [i*512, (i+1)*512) on the 16 kHz axis.
    let mut vad = AccumulatingVad::new(VAD_SAMPLE_RATE)?;
    let threshold = vad.threshold();

    let mut probs: Vec<f32> = Vec::with_capacity(mono16k.len() / CHUNK_SIZE + 1);
    let mut rms: Vec<f32> = Vec::with_capacity(probs.capacity());
    let mut peak: Vec<f32> = Vec::with_capacity(probs.capacity());
    for chunk in mono16k.chunks(CHUNK_SIZE) {
        if chunk.len() < CHUNK_SIZE {
            break; // trailing partial chunk carries no full inference
        }
        probs.push(vad.feed(chunk));
        let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
        rms.push((sum_sq / chunk.len() as f32).sqrt());
        peak.push(chunk.iter().fold(0.0f32, |m, s| m.max(s.abs())));
    }

    let atlas = serde_json::json!({
        "source_wav": wav,
        "capture_sample_rate": capture_rate,
        "capture_samples": capture_len,
        "vad_sample_rate": VAD_SAMPLE_RATE,
        "chunk_samples": CHUNK_SIZE,
        "threshold": threshold,
        "chunks": probs.len(),
        "probs": probs,
        "rms": rms,
        "peak": peak,
    });
    std::fs::write(&out, serde_json::to_vec(&atlas)?)?;
    eprintln!(
        "vad_atlas_probe: {} chunks ({:.1}s) -> {}",
        probs.len(),
        probs.len() as f32 * CHUNK_SIZE as f32 / VAD_SAMPLE_RATE as f32,
        out
    );
    Ok(())
}
