//! Audio output module - playback and WAV export.
//!
//! Provides audio playback via cpal and WAV file export via hound.
//! Used by TTS module to output synthesized speech.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{SampleFormat, WavSpec, WavWriter};
use tracing::{debug, info, warn};

/// Audio player for TTS output
///
/// Wraps cpal for cross-platform audio playback.
pub struct AudioPlayer {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    is_dummy: bool,
}

impl AudioPlayer {
    /// Create a new audio player with default output device
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("No audio output device available"))?;

        let config = device
            .default_output_config()
            .context("Failed to get default output config")?;

        debug!(
            "Audio player initialized: {:?} ({:?})",
            device.description(),
            config.sample_format()
        );

        Ok(Self {
            device,
            config,
            is_dummy: false,
        })
    }

    /// Create a dummy player that does nothing (for when audio init fails)
    pub fn dummy() -> Self {
        warn!("Creating dummy audio player - playback will be silent");
        Self {
            device: cpal::default_host()
                .default_output_device()
                .expect("Dummy needs at least one device"),
            config: cpal::default_host()
                .default_output_device()
                .and_then(|d| d.default_output_config().ok())
                .expect("Dummy needs config"),
            is_dummy: true,
        }
    }

    /// Play audio samples (blocking until complete)
    ///
    /// Resamples if necessary to match device sample rate.
    pub fn play(&self, samples: &[f32], sample_rate: u32) -> Result<()> {
        if self.is_dummy {
            warn!(
                "Dummy player: skipping playback of {} samples",
                samples.len()
            );
            return Ok(());
        }

        if samples.is_empty() {
            return Ok(());
        }

        let device_rate = self.config.sample_rate();
        let samples = if sample_rate != device_rate {
            debug!("Resampling from {}Hz to {}Hz", sample_rate, device_rate);
            resample(samples, sample_rate, device_rate)
        } else {
            samples.to_vec()
        };

        let samples = Arc::new(samples);
        let position = Arc::new(Mutex::new(0usize));
        let finished = Arc::new((Mutex::new(false), Condvar::new()));

        let samples_clone = Arc::clone(&samples);
        let position_clone = Arc::clone(&position);
        let finished_clone = Arc::clone(&finished);

        let channels = self.config.channels() as usize;

        let stream = match self.config.sample_format() {
            cpal::SampleFormat::F32 => {
                self.build_stream::<f32>(samples_clone, position_clone, finished_clone, channels)?
            }
            cpal::SampleFormat::I16 => {
                self.build_stream::<i16>(samples_clone, position_clone, finished_clone, channels)?
            }
            cpal::SampleFormat::U16 => {
                self.build_stream::<u16>(samples_clone, position_clone, finished_clone, channels)?
            }
            _ => return Err(anyhow!("Unsupported sample format")),
        };

        stream.play().context("Failed to start audio stream")?;

        // Wait for playback to complete.
        // Poison-recovery on the playback wait: a panic elsewhere holding this
        // lock must not turn into a permanent deadlock/abort on the audio path;
        // recover the inner guard and continue waiting on the condvar.
        let (lock, cvar) = &*finished;
        let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
        while !*done {
            done = cvar.wait(done).unwrap_or_else(|e| e.into_inner());
        }

        debug!("Playback complete");
        Ok(())
    }

    /// Build output stream for specific sample type
    fn build_stream<T>(
        &self,
        samples: Arc<Vec<f32>>,
        position: Arc<Mutex<usize>>,
        finished: Arc<(Mutex<bool>, Condvar)>,
        channels: usize,
    ) -> Result<cpal::Stream>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let config = self.config.config();

        let stream = self.device.build_output_stream(
            &config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Poison-recovery inside the real-time audio callback: a panic
                // must not poison the position lock and silence all further
                // playback; recover the inner guard and keep feeding samples.
                let mut pos = position.lock().unwrap_or_else(|e| e.into_inner());
                let samples_len = samples.len();

                for frame in data.chunks_mut(channels) {
                    let sample = if *pos < samples_len {
                        samples[*pos]
                    } else {
                        0.0
                    };

                    let sample_t: T = T::from_sample(sample);
                    for out in frame.iter_mut() {
                        *out = sample_t;
                    }

                    if *pos < samples_len {
                        *pos += 1;
                    }
                }

                // Signal completion when done
                if *pos >= samples_len {
                    let (lock, cvar) = &*finished;
                    let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
                    *done = true;
                    cvar.notify_one();
                }
            },
            |err| {
                warn!("Audio stream error: {}", err);
            },
            None,
        )?;

        Ok(stream)
    }

    /// Save audio samples to WAV file
    ///
    /// Saves as 32-bit float WAV at the specified sample rate.
    pub fn save_wav(samples: &[f32], sample_rate: u32, path: &Path) -> Result<()> {
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };

        let mut writer = WavWriter::create(path, spec)
            .with_context(|| format!("Failed to create WAV file: {}", path.display()))?;

        for &sample in samples {
            writer.write_sample(sample)?;
        }

        writer.finalize()?;

        info!(
            "Saved {} samples to {} ({:.2}s @ {}Hz)",
            samples.len(),
            path.display(),
            samples.len() as f32 / sample_rate as f32,
            sample_rate
        );

        Ok(())
    }

    /// Save audio samples to WAV file with 16-bit PCM format
    ///
    /// More compatible format for older players.
    pub fn save_wav_pcm16(samples: &[f32], sample_rate: u32, path: &Path) -> Result<()> {
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };

        let mut writer = WavWriter::create(path, spec)
            .with_context(|| format!("Failed to create WAV file: {}", path.display()))?;

        for &sample in samples {
            // Convert f32 [-1.0, 1.0] to i16 [-32768, 32767]
            let clamped = sample.clamp(-1.0, 1.0);
            let pcm16 = (clamped * 32767.0) as i16;
            writer.write_sample(pcm16)?;
        }

        writer.finalize()?;

        info!(
            "Saved {} samples (PCM16) to {} ({:.2}s @ {}Hz)",
            samples.len(),
            path.display(),
            samples.len() as f32 / sample_rate as f32,
            sample_rate
        );

        Ok(())
    }
}

/// Simple linear resampling
///
/// For production use, consider a proper resampling library like rubato.
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let new_len = (samples.len() as f64 * ratio).ceil() as usize;
    let mut resampled = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos.floor() as usize;
        let frac = src_pos - src_idx as f64;

        let sample = if src_idx + 1 < samples.len() {
            // Linear interpolation
            samples[src_idx] * (1.0 - frac as f32) + samples[src_idx + 1] * frac as f32
        } else if src_idx < samples.len() {
            samples[src_idx]
        } else {
            0.0
        };

        resampled.push(sample);
    }

    resampled
}

/// Normalize audio to target loudness (simple peak normalization)
pub fn normalize_audio(samples: &mut [f32], target_peak: f32) {
    let current_peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

    if current_peak > 0.0 && current_peak != target_peak {
        let scale = target_peak / current_peak;
        for sample in samples.iter_mut() {
            *sample *= scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_same_rate() {
        let samples = vec![0.0, 0.5, 1.0, 0.5, 0.0];
        let resampled = resample(&samples, 16000, 16000);
        assert_eq!(samples, resampled);
    }

    #[test]
    fn test_resample_upsample() {
        let samples = vec![0.0, 1.0];
        let resampled = resample(&samples, 16000, 32000);
        assert_eq!(resampled.len(), 4);
    }

    #[test]
    fn test_normalize() {
        let mut samples = vec![0.0, 0.25, 0.5, 0.25, 0.0];
        normalize_audio(&mut samples, 1.0);
        assert!((samples[2] - 1.0).abs() < 0.001);
    }
}
