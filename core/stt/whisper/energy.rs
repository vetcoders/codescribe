//! File-level energy timeline from a Whisper log-mel spectrogram.
//!
//! Candle's `pcm_to_mel` already returns log-compressed, max-relative values
//! laid out `[n_mels][n_frames]`. This module reduces that tensor to two
//! per-frame means (all bins, and the ~300–3000 Hz voice band) and draws an
//! 8-level sparkline. The hop is the Whisper hop (160 samples = 10 ms at 16 kHz).

use crate::pipeline::contracts::EnergyTimeline;

const SPARKLINE_BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const MEL_HZ_MIN: f64 = 0.0;
const MEL_HZ_MAX: f64 = 8_000.0;
const VOICE_HZ_LO: f64 = 300.0;
const VOICE_HZ_HI: f64 = 3_000.0;

fn hz_to_mel(hz: f64) -> f64 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

/// Mel-bin range whose centre frequency sits in ~300–3000 Hz on the 0–8000 Hz scale.
///
/// Computed from `n_mels` each call (80 or 128 in product models). No table.
pub fn voice_bins(n_mels: usize) -> std::ops::Range<usize> {
    if n_mels == 0 {
        return 0..0;
    }
    let mel_lo = hz_to_mel(MEL_HZ_MIN);
    let mel_hi = hz_to_mel(MEL_HZ_MAX);
    let span = mel_hi - mel_lo;
    if span <= 0.0 {
        return 0..0;
    }
    let voice_mel_lo = hz_to_mel(VOICE_HZ_LO);
    let voice_mel_hi = hz_to_mel(VOICE_HZ_HI);
    let mut start = None;
    let mut end = 0;
    for bin in 0..n_mels {
        let centre = mel_lo + (bin as f64 + 0.5) / n_mels as f64 * span;
        if centre >= voice_mel_lo && centre <= voice_mel_hi {
            if start.is_none() {
                start = Some(bin);
            }
            end = bin + 1;
        }
    }
    match start {
        Some(s) if s < end => s..end,
        _ => 0..0,
    }
}

fn frame_mean(mel: &[f32], n_frames: usize, bins: std::ops::Range<usize>, frame: usize) -> f32 {
    let count = bins.end.saturating_sub(bins.start);
    if count == 0 || n_frames == 0 || frame >= n_frames {
        return 0.0;
    }
    let mut sum = 0.0_f32;
    for bin in bins {
        sum += mel[bin * n_frames + frame];
    }
    sum / count as f32
}

/// Per-frame mean energy over all bins and over the voice-band bins.
///
/// `mel` is candle's `[n_mels][n_frames]` log-mel. Values are already
/// log-compressed; they are not re-logged. `hop_ms` is stored as given.
pub fn energy_timeline(mel: &[f32], n_mels: usize, hop_ms: u16) -> EnergyTimeline {
    if n_mels == 0 || mel.is_empty() {
        return EnergyTimeline {
            hop_ms,
            frames: Vec::new(),
            voice: Vec::new(),
        };
    }
    let n_frames = mel.len() / n_mels;
    if n_frames == 0 {
        return EnergyTimeline {
            hop_ms,
            frames: Vec::new(),
            voice: Vec::new(),
        };
    }
    let voice = voice_bins(n_mels);
    let mut frames = Vec::with_capacity(n_frames);
    let mut voice_frames = Vec::with_capacity(n_frames);
    for frame in 0..n_frames {
        frames.push(frame_mean(mel, n_frames, 0..n_mels, frame));
        voice_frames.push(frame_mean(mel, n_frames, voice.clone(), frame));
    }
    EnergyTimeline {
        hop_ms,
        frames,
        voice: voice_frames,
    }
}

fn percentile_nearest(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = (p * (sorted.len() - 1) as f32).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Eight-level sparkline. Each bucket's mean is placed linearly between the
/// timeline's p5 and p95 so a single loud click does not flatten the row.
pub fn sparkline(values: &[f32], buckets: usize) -> String {
    if values.is_empty() || buckets == 0 {
        return String::new();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p5 = percentile_nearest(&sorted, 0.05);
    let p95 = percentile_nearest(&sorted, 0.95);
    let span = p95 - p5;
    let n = values.len();
    let mut out = String::with_capacity(buckets);
    for i in 0..buckets {
        let start = i * n / buckets;
        let end = (i + 1) * n / buckets;
        let mean = if start >= end {
            values[start.min(n - 1)]
        } else {
            let slice = &values[start..end];
            slice.iter().copied().sum::<f32>() / slice.len() as f32
        };
        let bar = if span <= f32::EPSILON {
            SPARKLINE_BARS[0]
        } else {
            let t = ((mean - p5) / span).clamp(0.0, 1.0);
            SPARKLINE_BARS[(t * 7.0).round() as usize]
        };
        out.push(bar);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_mel(n_mels: usize, n_frames: usize, hot: std::ops::Range<usize>) -> Vec<f32> {
        let mut mel = vec![-1.0_f32; n_mels * n_frames];
        for frame in hot {
            for bin in 0..n_mels {
                mel[bin * n_frames + frame] = 1.0;
            }
        }
        mel
    }

    #[test]
    fn sparkline_peaks_on_hot_frames_10_to_20() {
        let mel = synthetic_mel(80, 32, 10..21);
        let timeline = energy_timeline(&mel, 80, 10);
        assert_eq!(timeline.hop_ms, 10);
        assert_eq!(timeline.frames.len(), 32);
        assert_eq!(timeline.voice.len(), 32);
        let row = sparkline(&timeline.frames, 32);
        assert_eq!(row.chars().count(), 32);
        for (i, ch) in row.chars().enumerate() {
            if (10..21).contains(&i) {
                assert_eq!(ch, '█', "expected peak at frame {i}, got {ch}");
            } else {
                assert_eq!(ch, '▁', "expected floor at frame {i}, got {ch}");
            }
        }
    }

    #[test]
    fn voice_bins_eighty_and_one_twenty_eight_are_inside_range() {
        let bins80 = voice_bins(80);
        assert!(!bins80.is_empty());
        assert!(bins80.start < 80);
        assert!(bins80.end <= 80);
        let bins128 = voice_bins(128);
        assert!(!bins128.is_empty());
        assert!(bins128.start < 128);
        assert!(bins128.end <= 128);
    }

    #[test]
    fn empty_mel_yields_empty_vectors() {
        let empty = energy_timeline(&[], 80, 10);
        assert!(empty.frames.is_empty());
        assert!(empty.voice.is_empty());
        let zero_bins = energy_timeline(&[0.5, 0.5], 0, 10);
        assert!(zero_bins.frames.is_empty());
        assert!(zero_bins.voice.is_empty());
    }

    #[test]
    fn sparkline_empty_input_is_empty() {
        assert!(sparkline(&[], 10).is_empty());
    }
}
