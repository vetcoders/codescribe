//! Decode audio files to mono `f32` samples, and resample them to the 16 kHz
//! the speech models expect.
//!
//! Container and codec detection is Symphonia's job, so WAV, MP3, m4a and the
//! rest arrive through one entry point. Every sample format Symphonia can hand
//! back is downmixed to mono by averaging channels — the models are monophonic,
//! so discarding the stereo image early keeps every later stage simpler.

use anyhow::{Result, anyhow};
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::conv::FromSample;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;

use crate::safe_path;

/// Decode an audio file to `(mono samples, sample rate)`.
///
/// The path goes through [`safe_path::safe_open`] first, so this cannot be
/// pointed at somewhere it should not read. Sample rate is taken from the
/// first decoded packet; an end-of-stream ends the loop normally, while any
/// other decode error aborts with context.
pub fn load_audio_file(path: &Path) -> Result<(Vec<f32>, u32)> {
    let src = safe_path::safe_open(path)?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .map_err(|e| anyhow!("Failed to probe audio format: {}", e))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("No supported audio track found"))?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .map_err(|e| anyhow!("Failed to create decoder: {}", e))?;

    let track_id = track.id;
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(anyhow!("Failed to decode packet: {}", e)),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_rate == 0 {
                    sample_rate = decoded.spec().rate;
                }

                match decoded {
                    AudioBufferRef::F32(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += buf.chan(ch)[i];
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::U8(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::U16(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::U24(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::U32(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::S8(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::S16(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::S24(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::S32(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                    AudioBufferRef::F64(buf) => {
                        let channels = buf.spec().channels.count();
                        let frames = buf.frames();
                        for i in 0..frames {
                            let mut sum = 0.0f32;
                            for ch in 0..channels {
                                sum += f32::from_sample(buf.chan(ch)[i]);
                            }
                            samples.push(sum / channels as f32);
                        }
                    }
                }
            }
            Err(e) => return Err(anyhow!("Failed to decode audio frame: {}", e)),
        }
    }

    Ok((samples, sample_rate))
}

/// Resample complete model-input audio to 16 kHz with a windowed-sinc filter.
///
/// Audio already at 16 kHz (and empty or rate-less input) is returned
/// untouched. Downsampling removes out-of-band energy before decimation.
/// This stateless conversion is for complete clips, not consecutive capture
/// packets: the original capture samples remain the ledger's coordinates.
pub fn resample_to_16k(samples: &[f32], original_rate: u32) -> Vec<f32> {
    if samples.is_empty() || original_rate == 0 || original_rate == 16000 {
        return samples.to_vec();
    }

    const RADIUS: isize = 96;
    let cutoff = (16000.0 / f64::from(original_rate)).min(1.0) * 0.94;
    let new_len = (samples.len() as u128 * 16000).div_ceil(u128::from(original_rate)) as usize;
    let mut output = Vec::with_capacity(new_len);
    let mut kernels = std::collections::HashMap::new();
    let max_idx = samples.len() - 1;
    for i in 0..new_len {
        // Integer phase accounting avoids drift on long 44.1 kHz recordings.
        let position = i as u128 * u128::from(original_rate);
        let center = (position / 16000) as usize;
        let phase = (position % 16000) as u32;
        let kernel = kernels
            .entry(phase)
            .or_insert_with(|| bandlimited_kernel(cutoff, f64::from(phase) / 16000.0, RADIUS));
        let mut sum = 0.0;
        for (tap, weight) in kernel.iter().enumerate() {
            let offset = tap as isize - RADIUS;
            let index = center.saturating_add_signed(offset).min(max_idx);
            sum += f64::from(samples[index]) * weight;
        }
        output.push(sum as f32);
    }
    output
}

fn bandlimited_kernel(cutoff: f64, phase: f64, radius: isize) -> Vec<f64> {
    let mut kernel: Vec<f64> = (-radius..=radius)
        .map(|offset| {
            let distance = offset as f64 - phase;
            let normalized = distance / radius as f64;
            if normalized.abs() >= 1.0 {
                return 0.0;
            }
            let angle = std::f64::consts::PI * cutoff * distance;
            let sinc = if angle.abs() < 1e-12 {
                1.0
            } else {
                angle.sin() / angle
            };
            let window_angle = std::f64::consts::PI * normalized;
            let window = 0.42 + 0.5 * window_angle.cos() + 0.08 * (2.0 * window_angle).cos();
            cutoff * sinc * window
        })
        .collect();
    let gain: f64 = kernel.iter().sum();
    for weight in &mut kernel {
        *weight /= gain;
    }
    kernel
}

#[cfg(test)]
mod resampling_tests {
    use super::resample_to_16k;

    fn tone(rate: u32, frequency: f64) -> Vec<f32> {
        (0..rate)
            .map(|i| {
                (std::f64::consts::TAU * frequency * f64::from(i) / f64::from(rate)).sin() as f32
            })
            .collect()
    }

    fn interior_rms(samples: &[f32]) -> f64 {
        let interior = &samples[512..samples.len() - 512];
        (interior.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / interior.len() as f64).sqrt()
    }

    #[test]
    fn rejects_out_of_band_energy_without_erasing_speech_band() {
        for rate in [44100, 48000, 96000] {
            for frequency in [1000.0, 6000.0] {
                let output = resample_to_16k(&tone(rate, frequency), rate);
                assert_eq!(output.len(), 16000);
                assert!((interior_rms(&output) - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01);
            }
            for frequency in [10000.0, 12000.0] {
                let output = resample_to_16k(&tone(rate, frequency), rate);
                assert!(
                    interior_rms(&output) < 0.001,
                    "alias rejection at {rate} Hz / tone {frequency}"
                );
            }
        }
    }

    #[test]
    fn preserves_dc_short_inputs_and_exact_duration() {
        for rate in [8000, 22050, 44100, 48000, 96000] {
            for length in [1usize, 2, 13, 1024, 44101] {
                let output = resample_to_16k(&vec![0.25; length], rate);
                assert_eq!(
                    output.len(),
                    (length as u128 * 16000).div_ceil(u128::from(rate)) as usize
                );
                assert!(output.iter().all(|v| (*v - 0.25).abs() < 1e-6));
            }
        }
    }

    #[test]
    fn keeps_existing_no_conversion_cases() {
        let input = [0.25, -0.5, 0.0, 1.0];
        assert_eq!(resample_to_16k(&input, 16000), input);
        assert_eq!(resample_to_16k(&input, 0), input);
        assert!(resample_to_16k(&[], 48000).is_empty());
    }

    #[test]
    fn impulse_has_no_added_group_delay() {
        let mut input = vec![0.0; 4800];
        input[2400] = 1.0;
        let output = resample_to_16k(&input, 48000);
        let peak = output
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
            .unwrap()
            .0;
        assert_eq!(peak, 800);
    }
}
