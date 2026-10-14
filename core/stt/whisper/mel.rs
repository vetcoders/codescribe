//! Whisper log-mel spectrogram, frame-identical to candle's `pcm_to_mel`.
//!
//! Candle pads every call with 1500 frames (15 s) of zeros, runs a recursive
//! FFT that recomputes `sin`/`cos` per element on each of them, and fans the
//! work out over up to twelve threads. A live 8 s window therefore spent
//! roughly two thirds of its work on silence and lit every core; sampled on
//! 2026-10-14 the mel front end alone held ~0.5–0.65 of a core for the whole
//! take (fans on long takes, on a laptop).
//!
//! A frame that holds no sample has the log-mel `log10(1e-10)` in every bin,
//! so only frames that touch real samples are transformed here, through a
//! planned real FFT, on the calling thread. Frame budget, zero fill, spectrum
//! fold, filter product and normalisation follow candle line for line, so the
//! encoder sees the same tensor.

use std::sync::{Arc, OnceLock};

use candle_transformers::models::whisper::{self as whisper, Config};
use realfft::{RealFftPlanner, RealToComplex};

/// `log10(1e-10)`: candle's floor for a frame whose spectrum sums to zero.
const ZERO_FRAME_LOG_MEL: f32 = -10.0;

struct Plan {
    hann: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
}

fn plan() -> &'static Plan {
    static PLAN: OnceLock<Plan> = OnceLock::new();
    PLAN.get_or_init(|| {
        let fft_size = whisper::N_FFT;
        let two_pi = 2.0 * std::f32::consts::PI;
        let hann = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (two_pi * i as f32 / fft_size as f32).cos()))
            .collect();
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(fft_size);
        Plan { hann, fft }
    })
}

/// Drop-in for `candle_transformers::models::whisper::audio::pcm_to_mel`.
///
/// `filters` is the `[num_mel_bins][1 + N_FFT / 2]` bank the engine loads
/// from the model directory. The result is laid out `[num_mel_bins][frames]`
/// with candle's frame budget: `samples / HOP_LENGTH` rounded up to a
/// multiple of 1500 frames, plus 1500 more.
pub fn pcm_to_mel(cfg: &Config, samples: &[f32], filters: &[f32]) -> Vec<f32> {
    log_mel(samples, filters, cfg.num_mel_bins)
}

fn log_mel(samples: &[f32], filters: &[f32], n_mel: usize) -> Vec<f32> {
    let fft_size = whisper::N_FFT;
    let hop = whisper::HOP_LENGTH;
    let n_fft = 1 + fft_size / 2;
    // Same frame budget as candle: pad to a multiple of 1500 frames, then one
    // more block of 1500. The consumers derive the frame count from the
    // output length, so the budget must not change.
    let pad = 100 * whisper::CHUNK_LENGTH / 2;
    let n_len = samples.len() / hop;
    let n_len = if n_len.is_multiple_of(pad) {
        n_len
    } else {
        (n_len / pad + 1) * pad
    };
    let n_len = n_len + pad;
    let mut mel = vec![ZERO_FRAME_LOG_MEL; n_len * n_mel];
    if n_mel == 0 || filters.len() < n_mel * n_fft {
        return finish(mel);
    }

    let plan = plan();
    let mut input = plan.fft.make_input_vec();
    let mut output = plan.fft.make_output_vec();
    let mut scratch = plan.fft.make_scratch_vec();
    let mut power = vec![0.0f32; n_fft];
    // A frame starting at or past the last sample is all zero fill; candle
    // transforms it anyway and lands on the floor above.
    let live_frames = samples.len().div_ceil(hop).min(n_len);
    for frame in 0..live_frames {
        let offset = frame * hop;
        let avail = (samples.len() - offset).min(fft_size);
        for (slot, (window, sample)) in input
            .iter_mut()
            .zip(plan.hann.iter().zip(&samples[offset..offset + avail]))
        {
            *slot = window * sample;
        }
        input[avail..].fill(0.0);
        plan.fft
            .process_with_scratch(&mut input, &mut output, &mut scratch)
            .expect("real FFT of a fixed-size frame");
        // |X|² with candle's fold: bins 1..N_FFT/2 collect both halves of the
        // spectrum, DC and Nyquist stand alone.
        for (slot, bin) in power.iter_mut().zip(&output) {
            *slot = bin.norm_sqr();
        }
        for slot in &mut power[1..fft_size / 2] {
            *slot *= 2.0;
        }
        for (bin, row) in filters.chunks_exact(n_fft).take(n_mel).enumerate() {
            let sum: f32 = row.iter().zip(&power).map(|(f, p)| f * p).sum();
            mel[bin * n_len + frame] = sum.max(1e-10).log10();
        }
    }
    finish(mel)
}

/// Candle's normalisation: clamp eight decades under the loudest bin, then
/// map into the encoder's range.
fn finish(mut mel: Vec<f32>) -> Vec<f32> {
    let mmax = mel.iter().copied().fold(f32::MIN, f32::max) - 8.0;
    for value in &mut mel {
        *value = value.max(mmax) / 4.0 + 1.0;
    }
    mel
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_transformers::models::whisper::audio::log_mel_spectrogram_;

    const N_MEL: usize = 80;

    /// Deterministic noise so the comparison never depends on a fixture file.
    fn lcg(seed: &mut u64) -> f32 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }

    fn voice_like(seconds: f32) -> Vec<f32> {
        let rate = whisper::SAMPLE_RATE as f32;
        let mut seed = 0x5eed_u64;
        (0..(seconds * rate) as usize)
            .map(|i| {
                let t = i as f32 / rate;
                let envelope = 0.5 + 0.5 * (t * 1.3).sin();
                let voice = (t * 2.0 * std::f32::consts::PI * 140.0).sin() * 0.4
                    + (t * 2.0 * std::f32::consts::PI * 1_250.0).sin() * 0.2
                    + (t * 2.0 * std::f32::consts::PI * 2_900.0).sin() * 0.1;
                envelope * voice + 0.02 * lcg(&mut seed)
            })
            .collect()
    }

    /// Non-negative triangular bank; the exact bank does not matter for
    /// equivalence, only that every bin sees several FFT bins.
    fn filters() -> Vec<f32> {
        let n_fft = 1 + whisper::N_FFT / 2;
        let mut bank = vec![0.0f32; N_MEL * n_fft];
        for (bin, row) in bank.chunks_exact_mut(n_fft).enumerate() {
            let centre = (bin + 1) as f32 * n_fft as f32 / (N_MEL + 2) as f32;
            let width = 2.0 + bin as f32 * 0.1;
            for (k, weight) in row.iter_mut().enumerate() {
                let distance = (k as f32 - centre).abs();
                *weight = (1.0 - distance / width).max(0.0) * 0.01;
            }
        }
        bank
    }

    fn candle(samples: &[f32], filters: &[f32]) -> Vec<f32> {
        log_mel_spectrogram_(
            samples,
            filters,
            whisper::N_FFT,
            whisper::HOP_LENGTH,
            N_MEL,
            false,
        )
    }

    fn assert_same(samples: &[f32]) {
        let bank = filters();
        let expected = candle(samples, &bank);
        let actual = log_mel(samples, &bank, N_MEL);
        assert_eq!(actual.len(), expected.len(), "frame budget differs");
        let worst = actual
            .iter()
            .zip(&expected)
            .map(|(a, e)| (a - e).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 1e-3,
            "log-mel drifts from candle by {worst} on {} samples",
            samples.len()
        );
    }

    #[test]
    fn an_eight_second_live_window_matches_candle_bin_for_bin() {
        assert_same(&voice_like(8.0));
    }

    #[test]
    fn a_window_not_aligned_to_the_hop_matches_candle() {
        // 0.37 s = 5920 samples: a partial last frame and a ragged tail.
        assert_same(&voice_like(0.37));
    }

    #[test]
    fn an_empty_window_matches_candle() {
        assert_same(&[]);
    }

    #[test]
    fn silence_beyond_the_samples_sits_on_candles_floor() {
        let samples = voice_like(1.0);
        let bank = filters();
        let mel = log_mel(&samples, &bank, N_MEL);
        let n_len = mel.len() / N_MEL;
        let live = samples.len().div_ceil(whisper::HOP_LENGTH);
        assert!(live < n_len, "the budget always leaves padded frames");
        // Every padded frame carries the same clamped floor, in every bin,
        // and nothing in the tensor sits below it.
        let floor = mel[live];
        let lowest = mel.iter().copied().fold(f32::MAX, f32::min);
        assert_eq!(floor, lowest);
        for bin in 0..N_MEL {
            for frame in live..n_len {
                assert_eq!(
                    mel[bin * n_len + frame],
                    floor,
                    "padded frame {frame} bin {bin}"
                );
            }
        }
    }
}
