//! Offline/test batch streaming transcription. Not part of the runtime
//! session path; compiled only for tests and the `offline_eval` feature
//! (the module is cfg-gated in `mod.rs`).

use anyhow::Result;
use tracing::{debug, info};

use crate::pipeline::dedup::dedup_chunk_overlap;
use crate::pipeline::stream_postprocess::StreamPostProcessor;
use crate::stt::whisper::singleton::transcribe_chunk;

use super::tuning::{env_bool_default, env_f32};

const DEFAULT_CHUNK_DURATION_SEC: f32 = 4.0;
const DEFAULT_OVERLAP_RATIO: f32 = 0.25; // 25% overlap for stronger context continuity

// ── Offline/test: batch streaming transcription ──────────────────────────────

/// Batch helper for offline evaluation on in-memory samples.
///
/// Not part of the runtime session path.
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

    let total_audio_sec = samples.len() as f32 / sample_rate as f32;
    let stride_sec = chunk_duration_sec - overlap_sec;
    let n_chunks =
        ((samples.len().saturating_sub(chunk_limit)) as f32 / step as f32).ceil() as usize + 1;
    let processing_factor = chunk_duration_sec / stride_sec;
    let effective_audio_sec = n_chunks as f32 * chunk_duration_sec;

    info!(
        "[STREAM_DIAG] chunk={:.1}s overlap={:.1}s stride={:.1}s | audio={:.1}s chunks={} factor={:.2}x effective={:.1}s",
        chunk_duration_sec,
        overlap_sec,
        stride_sec,
        total_audio_sec,
        n_chunks,
        processing_factor,
        effective_audio_sec
    );

    // Per-chunk engine acquisition via the singleton: chunks are independent
    // (overlap dedup happens on the text below), and routing through the
    // singleton keeps the idle-unload/reload bookkeeping consistent.
    let mut out = String::new();
    let mut offset = 0usize;
    let mut chunks_processed = 0usize;
    let t_start = std::time::Instant::now();

    while offset < samples.len() {
        let end = (offset + chunk_limit).min(samples.len());
        let chunk = &samples[offset..end];
        let chunk_sec = chunk.len() as f32 / sample_rate as f32;
        let t_chunk = std::time::Instant::now();
        let text = transcribe_chunk(chunk, sample_rate, language)?;
        let chunk_ms = t_chunk.elapsed().as_millis();
        chunks_processed += 1;

        debug!(
            "[STREAM_CHUNK] #{} offset={:.1}s len={:.1}s transcribe={}ms words={}",
            chunks_processed,
            offset as f32 / sample_rate as f32,
            chunk_sec,
            chunk_ms,
            text.split_whitespace().count()
        );

        if let Some(processor) = postprocessor.as_mut() {
            if let Some(cleaned) = processor.process(&text) {
                dedup_chunk_overlap(&mut out, &cleaned);
            }
        } else {
            dedup_chunk_overlap(&mut out, &text);
        }

        if end == samples.len() {
            break;
        }
        offset = offset.saturating_add(step);
    }

    let total_ms = t_start.elapsed().as_millis();
    info!(
        "[STREAM_DONE] chunks_processed={} total_ms={} out_words={}",
        chunks_processed,
        total_ms,
        out.split_whitespace().count()
    );

    // Optional: apply lexicon post-processing to streaming output.
    // Disabled by default; enable explicitly for offline-eval comparisons.
    if env_bool_default("CODESCRIBE_STREAM_LEXICON", false) && !out.trim().is_empty() {
        let mut lex = StreamPostProcessor::new();
        if let Some(cleaned) = lex.process(&out) {
            out = cleaned;
        }
    }

    Ok(out)
}

pub(crate) fn stream_chunk_duration_sec() -> f32 {
    env_f32("CODESCRIBE_STREAM_CHUNK_SEC", DEFAULT_CHUNK_DURATION_SEC).clamp(0.5, 30.0)
}

pub(crate) fn stream_overlap_sec(chunk_duration_sec: f32) -> f32 {
    let ratio = env_f32("CODESCRIBE_STREAM_OVERLAP_RATIO", DEFAULT_OVERLAP_RATIO).clamp(0.05, 0.8);
    (chunk_duration_sec * ratio).min(chunk_duration_sec * 0.8)
}
