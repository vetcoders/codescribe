//! A single cancellable output owner. No recorder or local synthesis engine.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Reserve the next utterance, cancelling any older synthesis/playback.
pub fn begin() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
}
/// Stop also invalidates pending synthesis, so late HTTP responses cannot play.
pub fn stop() {
    begin();
}
/// Whether this request still owns audio output.
pub fn current(ticket: u64) -> bool {
    GENERATION.load(Ordering::SeqCst) == ticket
}

/// Blocking playback runs on a blocking worker; stream construction and destruction
/// stay on that same thread. Returns false when superseded or explicitly stopped.
pub fn play(samples: Vec<f32>, sample_rate: u32, ticket: u64) -> anyhow::Result<bool> {
    use anyhow::{Context, bail};
    if !current(ticket) {
        return Ok(false);
    }
    let device = cpal::default_host()
        .default_output_device()
        .context("No audio output device")?;
    let config = device.default_output_config()?;
    let rate = config.sample_rate();
    let source = if rate == sample_rate {
        samples
    } else {
        let len = (samples.len() as f64 * f64::from(rate) / f64::from(sample_rate)).ceil() as usize;
        (0..len)
            .map(|i| {
                let pos = i as f64 * f64::from(sample_rate) / f64::from(rate);
                let a = pos as usize;
                let frac = (pos - a as f64) as f32;
                let first = samples.get(a).copied().unwrap_or_default();
                first + (samples.get(a + 1).copied().unwrap_or(first) - first) * frac
            })
            .collect()
    };
    let deadline =
        Instant::now() + Duration::from_secs_f64(source.len() as f64 / f64::from(rate) + 5.0);
    let state = Arc::new(AtomicU8::new(0));
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            stream::<f32>(&device, &config.config(), source, ticket, state.clone())?
        }
        cpal::SampleFormat::I16 => {
            stream::<i16>(&device, &config.config(), source, ticket, state.clone())?
        }
        cpal::SampleFormat::U16 => {
            stream::<u16>(&device, &config.config(), source, ticket, state.clone())?
        }
        _ => bail!("Unsupported audio output format"),
    };
    if !current(ticket) {
        return Ok(false);
    }
    stream.play()?;
    while current(ticket) && state.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    if !current(ticket) {
        return Ok(false);
    }
    match state.load(Ordering::Acquire) {
        1 => Ok(true),
        2 => bail!("Speech audio output failed"),
        _ => bail!("Speech audio output timed out"),
    }
}
fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: Vec<f32>,
    ticket: u64,
    state: Arc<AtomicU8>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let channels = usize::from(config.channels);
    let mut position = 0;
    let errors = state.clone();
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            // Complete only on the callback AFTER the last submitted buffer.
            // This avoids dropping the stream before that buffer reaches the device.
            if position >= samples.len() {
                state.store(1, Ordering::Release);
            }
            for frame in data.chunks_mut(channels) {
                let sample = if current(ticket) {
                    samples.get(position).copied().unwrap_or_default()
                } else {
                    0.0
                };
                position += 1;
                for out in frame {
                    *out = T::from_sample(sample);
                }
            }
        },
        move |_| {
            errors.store(2, Ordering::Release);
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_invalidates_pending_utterances() {
        let first = begin();
        assert!(current(first));
        let second = begin();
        assert!(!current(first));
        assert!(current(second));
        stop();
        assert!(!current(second));
    }
}
