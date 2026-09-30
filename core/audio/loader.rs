//! Decode audio files to mono `f32` samples, and resample them to the 16 kHz
//! the speech models expect.
//!
//! Container and codec detection is Symphonia's job, so WAV, MP3, m4a and the
//! rest arrive through one entry point. Every sample format Symphonia can hand
//! back is downmixed to mono by averaging channels — the models are monophonic,
//! so discarding the stereo image early keeps every later stage simpler.

use anyhow::{Result, anyhow};
use std::path::Path;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

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
        .probe(&hint, mss, Default::default(), Default::default())
        .map_err(|e| anyhow!("Failed to probe audio format: {}", e))?;

    let mut format = probed;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.as_ref().and_then(|p| p.audio()).is_some())
        .ok_or_else(|| anyhow!("No supported audio track found"))?;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(
            track
                .codec_params
                .as_ref()
                .and_then(|p| p.audio())
                .ok_or_else(|| anyhow!("Audio track has no codec parameters"))?,
            &Default::default(),
        )
        .map_err(|e| anyhow!("Failed to create decoder: {}", e))?;

    let track_id = track.id;
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(anyhow!("Failed to decode packet: {}", e)),
        };

        if packet.track_id != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_rate == 0 {
                    sample_rate = decoded.spec().rate();
                }

                let channels = decoded.spec().channels().count();
                if channels == 0 {
                    return Err(anyhow!("Decoded audio has no channels"));
                }
                let mut interleaved = vec![0.0f32; decoded.samples_interleaved()];
                decoded.copy_to_slice_interleaved(&mut interleaved);
                samples.extend(
                    interleaved
                        .chunks_exact(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32),
                );
            }
            Err(e) => return Err(anyhow!("Failed to decode audio frame: {}", e)),
        }
    }

    Ok((samples, sample_rate))
}

/// One PCM window sliced out of a retained take WAV, on the capture clock.
pub struct WavWindow {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    /// Absolute capture-clock index of `samples[0]` after padding and clamping.
    pub window_start: u64,
}

/// Read a mono 16-bit WAV and cut `[sample_start, sample_end)` plus `pad_ms`
/// of context on both sides, clamped to the file.
///
/// The retained take audio (`sessions/<id>.wav`) is contiguous mono i16 at the
/// capture rate, so capture-clock sample offsets map directly onto file
/// offsets — the same mapping the occurrence slots journal uses. Anything
/// else (wrong format, empty window past the end of the take) is an honest
/// error, never a silent trim to something the caller did not ask for.
pub fn slice_wav_i16(
    path: &Path,
    sample_start: u64,
    sample_end: u64,
    pad_ms: u32,
) -> Result<WavWindow> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| anyhow!("Failed to open WAV {}: {}", path.display(), e))?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err(anyhow!(
            "WAV {} is not mono 16-bit PCM (channels={}, bits={})",
            path.display(),
            spec.channels,
            spec.bits_per_sample
        ));
    }
    if sample_end <= sample_start {
        return Err(anyhow!(
            "empty sample window [{sample_start}, {sample_end})"
        ));
    }
    let total = reader.duration() as u64;
    if sample_start >= total {
        return Err(anyhow!(
            "sample window [{sample_start}, {sample_end}) starts past the take ({total} samples)"
        ));
    }
    let pad = (spec.sample_rate as u64 * pad_ms as u64) / 1000;
    let window_start = sample_start.saturating_sub(pad);
    let window_end = (sample_end.saturating_add(pad)).min(total);
    let mut samples = Vec::with_capacity((window_end - window_start) as usize);
    reader
        .seek(window_start as u32)
        .map_err(|e| anyhow!("Failed to seek WAV {}: {}", path.display(), e))?;
    for sample in reader
        .samples::<i16>()
        .take((window_end - window_start) as usize)
    {
        samples.push(sample.map_err(|e| anyhow!("Failed to decode WAV sample: {e}"))?);
    }
    Ok(WavWindow {
        samples,
        sample_rate: spec.sample_rate,
        window_start,
    })
}

/// Write mono 16-bit samples as a WAV file (the clip format the overlay plays).
pub fn write_wav_i16(path: &Path, samples: &[i16], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| anyhow!("Failed to create WAV {}: {}", path.display(), e))?;
    for &sample in samples {
        writer
            .write_sample(sample)
            .map_err(|e| anyhow!("Failed to write WAV sample: {e}"))?;
    }
    writer
        .finalize()
        .map_err(|e| anyhow!("Failed to finalize WAV {}: {}", path.display(), e))
}

/// Resample to 16 kHz by linear interpolation.
///
/// Audio already at 16 kHz (and empty or rate-less input) is returned
/// untouched, so the common path costs nothing but a copy. Linear
/// interpolation is deliberately cheap rather than band-limited: speech
/// recognition tolerates the aliasing, and the alternative would cost more
/// than the models gain.
pub fn resample_to_16k(samples: &[f32], original_rate: u32) -> Vec<f32> {
    if samples.is_empty() || original_rate == 0 {
        return samples.to_vec();
    }

    if original_rate == 16000 {
        return samples.to_vec();
    }

    // Simple linear interpolation for now
    let ratio = 16000.0 / original_rate as f32;
    let new_len = (samples.len() as f32 * ratio).ceil() as usize;
    let mut output = Vec::with_capacity(new_len);

    let max_idx = samples.len() - 1;

    for i in 0..new_len {
        let old_idx = (i as f32 / ratio).min(max_idx as f32);
        let idx0 = old_idx.floor() as usize;

        if idx0 >= max_idx {
            // Clamp to last sample to avoid out-of-bounds due to rounding
            output.push(samples[max_idx]);
            continue;
        }

        let idx1 = (idx0 + 1).min(max_idx);
        let t = old_idx - idx0 as f32;

        let s0 = samples[idx0];
        let s1 = samples[idx1];
        output.push(s0 * (1.0 - t) + s1 * t);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_integer_stereo_to_mono_with_original_sample_rate() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec)?;
        for sample in [16384i16, -16384, 8192, 24576] {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
        let (samples, rate) = load_audio_file(&path)?;
        assert_eq!(rate, 48_000);
        assert_eq!(samples, vec![0.0, 0.5]);
        Ok(())
    }

    #[test]
    fn decodes_float_mono_without_clipping_or_resampling() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("float.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let expected = [0.25f32, -0.5, 1.25];
        let mut writer = hound::WavWriter::create(&path, spec)?;
        for sample in expected {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
        let (samples, rate) = load_audio_file(&path)?;
        assert_eq!(rate, 16_000);
        assert_eq!(samples, expected);
        Ok(())
    }

    #[test]
    fn refuses_unrecognized_audio() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("invalid.wav");
        std::fs::write(&path, b"not a wave file")?;
        assert!(load_audio_file(&path).is_err());
        Ok(())
    }

    #[test]
    fn slice_maps_capture_clock_offsets_with_padding() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("take.wav");
        let pcm: Vec<i16> = (0..48_000).map(|i| (i % 1000) as i16).collect();
        write_wav_i16(&path, &pcm, 48_000)?;

        // 48 kHz × 150 ms = 7200 samples of padding on each side.
        let window = slice_wav_i16(&path, 10_000, 20_000, 150)?;
        assert_eq!(window.sample_rate, 48_000);
        assert_eq!(window.window_start, 2_800);
        assert_eq!(window.samples.len(), (27_200 - 2_800) as usize);
        assert_eq!(window.samples[0], pcm[2_800]);
        assert_eq!(*window.samples.last().unwrap(), pcm[27_199]);
        Ok(())
    }

    #[test]
    fn slice_clamps_padding_to_take_edges() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("take.wav");
        write_wav_i16(&path, &[7i16; 10_000], 16_000)?;

        let head = slice_wav_i16(&path, 100, 200, 1_000)?;
        assert_eq!(head.window_start, 0);
        assert_eq!(head.samples.len(), 10_000);
        let tail = slice_wav_i16(&path, 9_900, 10_000, 1_000)?;
        assert_eq!(tail.samples.len(), 10_000 - tail.window_start as usize);
        assert_eq!(head.sample_rate, 16_000);
        Ok(())
    }

    #[test]
    fn slice_refuses_empty_or_out_of_take_windows() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("take.wav");
        write_wav_i16(&path, &[1i16; 1_000], 16_000)?;
        assert!(slice_wav_i16(&path, 500, 500, 0).is_err());
        assert!(slice_wav_i16(&path, 2_000, 3_000, 0).is_err());
        Ok(())
    }

    #[test]
    fn slice_refuses_non_mono_i16_and_roundtrips_written_clip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let stereo = dir.path().join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&stereo, spec)?;
        writer.write_sample(1i16)?;
        writer.write_sample(-1i16)?;
        writer.finalize()?;
        assert!(slice_wav_i16(&stereo, 0, 1, 0).is_err());

        let clip = dir.path().join("clip.wav");
        write_wav_i16(&clip, &[1, -2, 3], 48_000)?;
        let window = slice_wav_i16(&clip, 0, 3, 0)?;
        assert_eq!(window.samples, vec![1, -2, 3]);
        Ok(())
    }
}
