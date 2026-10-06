//! Bounded recorder PCM on the original capture sample clock.
//!
//! The work plan selects integer sample bounds independently of recognizer
//! phrases. Retention never concatenates silence or silently crops a request.
//! Evicted or future samples refuse the whole range; capture time keeps advancing
//! after eviction. Seconds are only readouts derived from the original rate.

use std::collections::VecDeque;

/// How much audio one session keeps available for boundary resolution.
///
/// Sized for tail-patch reach, not for the whole session: 120 s at 16 kHz mono
/// f32 is ~7.7 MB, which is the ceiling we are willing to hold per session.
pub(crate) const DEFAULT_RETENTION_SECS: f32 = 120.0;

/// Bounded ring of session PCM with an absolute capture-sample clock.
pub(crate) struct LiveAudioBuffer {
    /// Capture rate, and the unit second↔index conversions are expressed in.
    #[cfg(test)]
    sample_rate: u32,
    /// Retained tail, oldest first.
    samples: VecDeque<f32>,
    /// Absolute session-sample index of `samples.front()`.
    start_index: u64,
    /// Absolute session-sample index one past `samples.back()` — i.e. the total
    /// number of samples ever pushed, retained or evicted.
    end_index: u64,
    /// Retention ceiling in samples.
    capacity: usize,
}

/// One resolved retained window with its canonical session-sample identity.
pub(crate) struct ResolvedAudioWindow {
    pub(crate) samples: Vec<f32>,
    pub(crate) sample_start: u64,
    pub(crate) sample_end: u64,
}

impl LiveAudioBuffer {
    /// Build a buffer for `sample_rate`, retaining at most `retention_secs`.
    pub(crate) fn new(sample_rate: u32, retention_secs: f32) -> Self {
        let rate = sample_rate.max(1);
        let capacity = if retention_secs.is_finite() && retention_secs > 0.0 {
            ((retention_secs as f64) * (rate as f64)).round().max(1.0) as usize
        } else {
            rate as usize
        };
        Self {
            #[cfg(test)]
            sample_rate: rate,
            samples: VecDeque::new(),
            start_index: 0,
            end_index: 0,
            capacity,
        }
    }

    /// Append one capture chunk, evicting the oldest samples past the cap.
    pub(crate) fn push(&mut self, chunk: &[f32]) {
        if chunk.is_empty() {
            return;
        }
        self.samples.extend(chunk.iter().copied());
        self.end_index = self.end_index.saturating_add(chunk.len() as u64);
        if self.samples.len() > self.capacity {
            let overflow = self.samples.len() - self.capacity;
            self.samples.drain(..overflow);
            self.start_index = self.start_index.saturating_add(overflow as u64);
        }
    }

    /// Retained sample count.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    /// Absolute sample index of the oldest retained sample.
    #[cfg(test)]
    pub(crate) fn retained_start_sample(&self) -> u64 {
        self.start_index
    }

    /// Session time of the oldest retained sample.
    #[cfg(test)]
    pub(crate) fn retained_start_secs(&self) -> f32 {
        self.start_index as f32 / self.sample_rate as f32
    }

    /// Total audio seen this session, retained or evicted.
    #[cfg(test)]
    pub(crate) fn session_secs(&self) -> f32 {
        self.end_index as f32 / self.sample_rate as f32
    }

    /// Total capture samples seen, retained or evicted.
    pub(crate) fn session_sample_end(&self) -> u64 {
        self.end_index
    }

    /// Cut `[sample_start, sample_end)` on the capture PCM clock.
    ///
    /// `None` when the range is inverted, evicted, or extends past capture.
    /// Both bounds address the capture sample clock directly.
    pub(crate) fn window_by_samples(
        &self,
        sample_start: u64,
        sample_end: u64,
    ) -> Option<ResolvedAudioWindow> {
        if sample_end < sample_start
            || sample_start < self.start_index
            || sample_start > self.end_index
            || sample_end > self.end_index
        {
            return None;
        }
        let lo = (sample_start - self.start_index) as usize;
        let hi = (sample_end - self.start_index) as usize;
        Some(ResolvedAudioWindow {
            samples: self.samples.range(lo..hi).copied().collect(),
            sample_start,
            sample_end,
        })
    }
}

/// Capture sample addressing, bounded retention, and unmodified PCM fixtures.
#[cfg(test)]
mod tests {
    use super::*;
    const RATE: u32 = 16_000;

    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| i as f32).collect()
    }

    fn push_chunked(buffer: &mut LiveAudioBuffer, samples: &[f32]) {
        for chunk in samples.chunks(1024) {
            buffer.push(chunk);
        }
    }

    #[test]
    fn requested_sample_span_keeps_original_coordinates_and_values() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &ramp(48_000));
        let window = buffer.window_by_samples(16_000, 32_000).unwrap();
        assert_eq!((window.sample_start, window.sample_end), (16_000, 32_000));
        assert_eq!(window.samples, ramp(48_000)[16_000..32_000]);
    }

    #[test]
    fn sample_bounds_cross_capture_chunk_boundaries_without_rounding() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &ramp(32_000));
        let window = buffer.window_by_samples(800, 2400).unwrap();
        assert_eq!(window.samples.len(), 1600);
        assert_eq!(window.samples[0], 800.0);
        assert_eq!(*window.samples.last().unwrap(), 2399.0);
    }

    #[test]
    fn eviction_refuses_old_pcm_and_does_not_reset_capture_clock() {
        let mut buffer = LiveAudioBuffer::new(RATE, 1.0);
        push_chunked(&mut buffer, &ramp(48_000));
        assert!(buffer.window_by_samples(0, 8000).is_none());
        assert_eq!(buffer.len(), 16_000);
        assert_eq!(buffer.retained_start_sample(), 32_000);
        assert_eq!(buffer.retained_start_secs(), 2.0);
        assert_eq!(buffer.session_sample_end(), 48_000);
        assert_eq!(buffer.session_secs(), 3.0);
        assert_eq!(
            buffer.window_by_samples(40_000, 48_000).unwrap().samples,
            ramp(48_000)[40_000..]
        );
    }

    #[test]
    fn even_one_sample_past_capture_is_refused_without_clamping() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        buffer.push(&ramp(16_000));
        for (start, end) in [
            (0, 16_001),
            (14_400, 16_800),
            (32_000, 48_000),
            (8000, 1600),
            (0, u64::MAX),
        ] {
            assert!(
                buffer.window_by_samples(start, end).is_none(),
                "{start}..{end}"
            );
        }
        assert_eq!(
            buffer
                .window_by_samples(14_400, 16_000)
                .unwrap()
                .samples
                .len(),
            1600
        );
        assert_eq!(buffer.len(), 16_000);
        assert_eq!(buffer.session_sample_end(), 16_000);
    }

    #[test]
    fn empty_capture_and_empty_chunks_do_not_invent_audio() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        buffer.push(&[]);
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.session_sample_end(), 0);
        assert!(buffer.window_by_samples(0, 1).is_none());
        assert!(buffer.window_by_samples(0, 0).unwrap().samples.is_empty());
    }

    #[test]
    fn fractional_retention_uses_each_original_capture_rate() {
        for rate in [100_u32, 16_000, 44_100, 48_000] {
            let mut buffer = LiveAudioBuffer::new(rate, 0.5);
            buffer.push(&ramp(rate as usize));
            let count = (f64::from(rate) * 0.5).round() as u64;
            assert_eq!(buffer.len() as u64, count);
            assert_eq!(buffer.session_sample_end(), u64::from(rate));
            assert_eq!(buffer.retained_start_sample(), u64::from(rate) - count);
        }
    }

    #[test]
    fn speech_and_silence_survive_cross_chunk_sample_addressing() {
        let mut pcm = vec![0.0; 96_000];
        for (start, end) in [(8000, 32_000), (40_000, 64_000), (70_400, 89_600)] {
            pcm[start..end].fill(0.25);
        }
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &pcm);
        let window = buffer.window_by_samples(4800, 91_200).unwrap();
        assert_eq!(window.samples, pcm[4800..91_200]);
        assert_eq!(window.samples.iter().filter(|v| **v > 0.0).count(), 67_200);
        assert_eq!(window.sample_start, 4800);
        assert_eq!(window.sample_end, 91_200);
    }
}

/// Finalized recorder-owned WAV receipt. Constructed after capture stops, never
/// in its callback. Identity and count are measured by the capture owner.
#[derive(Debug)]
pub struct FinalizedPcmArchive {
    pub(crate) session_id: String,
    pub(crate) capture_epoch: u64,
    pub(crate) sample_rate: u32,
    pub(crate) sample_count: u64,
    pub(crate) path: std::path::PathBuf,
}

pub(crate) struct OwnedTerminalPcm {
    samples: Vec<f32>,
}

impl FinalizedPcmArchive {
    pub(crate) fn load(
        self,
        session: &str,
        epoch: u64,
        rate: u32,
        count: u64,
    ) -> anyhow::Result<OwnedTerminalPcm> {
        anyhow::ensure!(
            self.session_id == session
                && self.capture_epoch == epoch
                && self.sample_rate == rate
                && self.sample_count == count,
            "terminal archive identity/rate/sample-count mismatch"
        );
        let reader = hound::WavReader::open(&self.path)?;
        let spec = reader.spec();
        anyhow::ensure!(
            spec.channels == 1
                && spec.sample_rate == rate
                && spec.bits_per_sample == 16
                && spec.sample_format == hound::SampleFormat::Int,
            "terminal archive must be recorder-native mono i16 at capture rate"
        );
        anyhow::ensure!(
            u64::from(reader.duration()) == count,
            "terminal archive WAV length mismatch"
        );
        // The recorder quantizes by multiplying by i16::MAX and truncating.
        // Decode on the same scale: sample positions are identical; values are
        // quantized, not claimed byte-identical to the live f32 samples.
        let samples = reader
            .into_samples::<i16>()
            .map(|sample| sample.map(|sample| f32::from(sample) / f32::from(i16::MAX)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(OwnedTerminalPcm { samples })
    }
}

impl OwnedTerminalPcm {
    pub(crate) fn window(&self, start: u64, end: u64) -> Option<ResolvedAudioWindow> {
        if end < start {
            return None;
        }
        let samples = self
            .samples
            .get(usize::try_from(start).ok()?..usize::try_from(end).ok()?)?;
        Some(ResolvedAudioWindow {
            samples: samples.to_vec(),
            sample_start: start,
            sample_end: end,
        })
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    #[test]
    fn archive_identity_and_i16_sample_clock_are_checked() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let pcm = [-1.0_f32, -0.25, 0.0, 0.25, 1.0];
        let mut wav = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in pcm {
            wav.write_sample((sample * i16::MAX as f32) as i16).unwrap();
        }
        wav.finalize().unwrap();
        let receipt = || FinalizedPcmArchive {
            session_id: "owned".into(),
            capture_epoch: 3,
            sample_rate: 48_000,
            sample_count: 5,
            path: file.path().into(),
        };
        assert!(receipt().load("foreign", 3, 48_000, 5).is_err());
        assert!(receipt().load("owned", 4, 48_000, 5).is_err());
        assert!(receipt().load("owned", 3, 16_000, 5).is_err());
        assert!(receipt().load("owned", 3, 48_000, 4).is_err());
        let owned = receipt().load("owned", 3, 48_000, 5).unwrap();
        assert!(owned.window(0, 6).is_none());
        let window = owned.window(1, 4).unwrap();
        assert_eq!((window.sample_start, window.sample_end), (1, 4));
        for (actual, expected) in window.samples.iter().zip(&pcm[1..4]) {
            assert!((actual - expected).abs() <= 1.0 / i16::MAX as f32);
        }
    }
}
