use anyhow::{anyhow, Result};
use std::path::Path;
use std::fs::File;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::conv::FromSample;

pub fn load_audio_file(path: &Path) -> Result<(Vec<f32>, u32)> {
    let src = File::open(path)?;
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
