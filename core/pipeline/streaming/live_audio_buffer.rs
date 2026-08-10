//! Bounded per-session PCM retention for the Apple progressive live path.
//!
//! The Apple path forwards captured PCM straight into one long-lived SFSpeech
//! request and then drops it. Nothing mid-session can re-read the audio, so
//! there is no window for Layer 1 tail-patch to hand to Whisper: a sealed
//! `UtteranceFinal` carries an `end_ts` but no way back to the samples it came
//! from.
//!
//! This buffer is that way back. It keeps a bounded, drop-oldest tail of the
//! session's samples indexed by absolute sample offset, so an utterance
//! boundary expressed in seconds resolves to the exact PCM span behind it.
//!
//! Contract notes:
//! - Time is **session time**: seconds since the first pushed sample, the same
//!   clock `apple_stream_worker` derives `audio_secs` from. It is NOT wall
//!   clock and NOT the capture device clock.
//! - `window` is honest about what it lost. A range that fell off the retention
//!   cap returns `None` rather than a silently short slice — a tail-patch fed a
//!   truncated window would rewrite committed canvas from the wrong audio.
//! - Non-finite bounds (NaN / ±inf) are rejected, never coerced. `f32 as u64`
//!   maps NaN to 0 and saturates infinities, which would turn a corrupt
//!   timestamp into a plausible-looking window. Same class as the non-finite
//!   `end_ts` guard on the stop boundary.

use std::collections::VecDeque;

/// How much audio one session keeps available for boundary resolution.
///
/// Sized for tail-patch reach, not for the whole session: 120 s at 16 kHz mono
/// f32 is ~7.7 MB, which is the ceiling we are willing to hold per session.
pub(crate) const DEFAULT_RETENTION_SECS: f32 = 120.0;

/// How far past the last captured sample an upper bound may land and still be
/// treated as chunk quantisation rather than a disagreeing clock.
///
/// A capture chunk is ~64 ms at 16 kHz, so a boundary reported at the very end
/// of the audio can round a fraction of a chunk past it. Anything beyond this
/// is not rounding — it is a timestamp that does not describe this session's
/// PCM, and truncating it would hand a caller audio that is not the span it
/// asked for. Deliberately well under the ±0.2 s boundary-mapping tolerance.
const CLAMP_TOLERANCE_SECS: f32 = 0.1;

/// Bounded ring of session PCM, addressable by session-time seconds.
pub(crate) struct LiveAudioBuffer {
    /// Capture rate, and the unit second↔index conversions are expressed in.
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

    /// Samples covering `[from_secs, to_secs)` in session time.
    ///
    /// `None` when the bounds are not finite, are inverted, start before what
    /// retention still holds, start past the audio seen so far, or end more
    /// than `CLAMP_TOLERANCE_SECS` past it. Within that tolerance the upper
    /// bound is clamped — a boundary landing a rounding step past the last
    /// pushed chunk is quantisation, not missing audio.
    ///
    /// Refusing beats truncating: a short window looks like a success and would
    /// address the wrong audio.
    pub(crate) fn window(&self, from_secs: f32, to_secs: f32) -> Option<Vec<f32>> {
        let from = self.index_for(from_secs)?;
        let to = self.index_for(to_secs)?;
        if to < from || from < self.start_index || from > self.end_index {
            return None;
        }
        let to = if to <= self.end_index {
            to
        } else {
            let overshoot = to - self.end_index;
            let tolerance = ((CLAMP_TOLERANCE_SECS as f64) * (self.sample_rate as f64)).round();
            if (overshoot as f64) > tolerance {
                return None;
            }
            self.end_index
        };
        let lo = (from - self.start_index) as usize;
        let hi = (to - self.start_index) as usize;
        Some(self.samples.range(lo..hi).copied().collect())
    }

    /// Release everything before `secs` — audio already committed downstream
    /// can never be re-cut, so holding it is pure footprint.
    pub(crate) fn committed_through(&mut self, secs: f32) {
        let Some(index) = self.index_for(secs) else {
            return;
        };
        if index <= self.start_index {
            return;
        }
        let drop_n = ((index - self.start_index) as usize).min(self.samples.len());
        self.samples.drain(..drop_n);
        self.start_index = self.start_index.saturating_add(drop_n as u64);
        // `drain` keeps the allocation; hand it back once it dwarfs what we
        // actually hold, otherwise a long session pays peak footprint forever.
        if self.samples.capacity() > 4 * self.samples.len().max(1) {
            self.samples.shrink_to_fit();
        }
    }

    /// Retained sample count.
    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    /// Session time of the oldest retained sample.
    pub(crate) fn retained_start_secs(&self) -> f32 {
        self.start_index as f32 / self.sample_rate as f32
    }

    /// Total audio seen this session, retained or evicted.
    pub(crate) fn session_secs(&self) -> f32 {
        self.end_index as f32 / self.sample_rate as f32
    }

    /// Absolute session-sample index for a session-time second, or `None` when
    /// the value cannot address audio at all.
    fn index_for(&self, secs: f32) -> Option<u64> {
        if !secs.is_finite() || secs < 0.0 {
            return None;
        }
        let index = ((secs as f64) * (self.sample_rate as f64)).round();
        if !index.is_finite() || index < 0.0 || index > u64::MAX as f64 {
            return None;
        }
        Some(index as u64)
    }
}

/// Retention, window honesty, commit release, and F3 end_ts mapping fixtures.
#[cfg(test)]
mod tests {
    use super::*;

    /// Session sample rate used by all retention fixtures (16 kHz mono).
    const RATE: u32 = 16_000;
    /// Capture-sized push quantum so tests exercise chunk quantisation.
    const CHUNK: usize = 1024;

    /// Push `samples` through the buffer in capture-sized chunks, the way the
    /// worker receives them — so every test exercises chunk quantisation.
    fn push_chunked(buffer: &mut LiveAudioBuffer, samples: &[f32]) {
        for chunk in samples.chunks(CHUNK) {
            buffer.push(chunk);
        }
    }

    /// Ramp signal: sample at index `i` has value `i as f32`, so any slice
    /// identifies its own absolute offset.
    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| i as f32).collect()
    }

    /// Half-open `[1s, 2s)` resolves to exactly one second of ramp samples.
    #[test]
    fn window_returns_the_requested_span_at_session_rate() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &ramp(RATE as usize * 3));

        let window = buffer.window(1.0, 2.0).expect("retained range");
        assert_eq!(window.len(), RATE as usize);
        assert_eq!(window[0], RATE as f32);
        assert_eq!(*window.last().unwrap(), (2 * RATE - 1) as f32);
    }

    /// Window bounds need not land on CHUNK multiples (cross-chunk honesty).
    #[test]
    fn window_spans_capture_chunk_boundaries() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &ramp(RATE as usize * 2));

        // 0.05 s = 800 samples — deliberately not a CHUNK multiple.
        let window = buffer.window(0.05, 0.15).expect("retained range");
        assert_eq!(window.len(), 1600);
        assert_eq!(window[0], 800.0);
        assert_eq!(*window.last().unwrap(), 2399.0);
    }

    /// Cap drop-oldest: evicted ranges are `None`, not silently short.
    #[test]
    fn cap_evicts_oldest_and_evicted_ranges_report_none() {
        // 1 s cap, 3 s pushed — the first two seconds are gone.
        let mut buffer = LiveAudioBuffer::new(RATE, 1.0);
        push_chunked(&mut buffer, &ramp(RATE as usize * 3));

        assert!(
            buffer.window(0.0, 0.5).is_none(),
            "evicted range must be None"
        );
        assert!(buffer.len() <= RATE as usize);
        assert_eq!(buffer.retained_start_secs(), 2.0);
        assert_eq!(buffer.session_secs(), 3.0);

        let window = buffer.window(2.5, 3.0).expect("retained tail");
        assert_eq!(window.len(), RATE as usize / 2);
        assert_eq!(window[0], (RATE as f32) * 2.5);
    }

    /// `committed_through` frees retained PCM that can never be re-cut.
    #[test]
    fn committed_through_releases_retained_samples() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &ramp(RATE as usize * 4));
        assert_eq!(buffer.len(), RATE as usize * 4);

        buffer.committed_through(3.0);

        assert_eq!(buffer.len(), RATE as usize);
        assert!(
            buffer.window(2.0, 2.5).is_none(),
            "committed audio is released"
        );
        let window = buffer.window(3.5, 4.0).expect("uncommitted tail retained");
        assert_eq!(window[0], (RATE as f32) * 3.5);
    }

    /// Same class as the non-finite `end_ts` guard on the stop boundary: a
    /// corrupt timestamp must refuse to address audio, not silently resolve to
    /// sample 0 (NaN) or the whole session (inf) through an `as u64` cast.
    #[test]
    fn non_finite_and_negative_bounds_are_refused() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &ramp(RATE as usize));

        assert!(buffer.window(f32::NAN, 0.5).is_none());
        assert!(buffer.window(0.0, f32::NAN).is_none());
        assert!(buffer.window(f32::INFINITY, 0.5).is_none());
        assert!(buffer.window(0.0, f32::NEG_INFINITY).is_none());
        assert!(buffer.window(-1.0, 0.5).is_none());
        assert!(buffer.window(0.5, 0.1).is_none(), "inverted range");
        // A corrupt bound must not prune the buffer either.
        buffer.committed_through(f32::NAN);
        assert_eq!(buffer.len(), RATE as usize);
    }

    /// Past-end: small overshoot clamps; large overshoot / past start refuse.
    #[test]
    fn window_past_the_audio_seen_so_far_is_refused_but_the_edge_is_clamped() {
        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &ramp(RATE as usize));

        // Starts beyond anything captured — nothing honest to return.
        assert!(buffer.window(2.0, 3.0).is_none());
        // Ends within a chunk-quantisation step of the last sample — clamp.
        let window = buffer.window(0.9, 1.05).expect("clamped to audio seen");
        assert_eq!(window.len(), RATE as usize / 10);
        // Ends well past the audio seen — that is a disagreeing clock, not
        // rounding. A short window here would silently mis-address audio.
        assert!(
            buffer.window(0.9, 1.5).is_none(),
            "overshoot beyond chunk tolerance must refuse, not truncate"
        );
    }

    // ── F3 falsification: Apple `end_ts` → PCM window mapping ───────────────

    /// Root-mean-square of a slice — the speech/silence discriminator used by
    /// the boundary fixture below.
    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// Session-time offsets, within `window`, of the first and last 10 ms block
    /// carrying speech-level energy.
    fn speech_edges_secs(window: &[f32]) -> Option<(f32, f32)> {
        let block = RATE as usize / 100; // 10 ms
        let mut first = None;
        let mut last = None;
        for (i, chunk) in window.chunks(block).enumerate() {
            if rms(chunk) > 0.05 {
                first.get_or_insert(i);
                last = Some(i);
            }
        }
        let (first, last) = (first?, last?);
        Some((
            first as f32 * block as f32 / RATE as f32,
            (last + 1) as f32 * block as f32 / RATE as f32,
        ))
    }

    /// Synthesise a session whose speech spans are known exactly: a 220 Hz tone
    /// inside each span, digital silence outside.
    fn fixture_session(total_secs: f32, speech_spans: &[(f32, f32)]) -> Vec<f32> {
        let total = (total_secs * RATE as f32) as usize;
        (0..total)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                if speech_spans.iter().any(|&(a, b)| t >= a && t < b) {
                    (std::f32::consts::TAU * 220.0 * t).sin() * 0.5
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// F3 (go/no-go for W2-A): an utterance boundary expressed as `end_ts`
    /// must resolve, via `window(prev_end, end_ts)`, to the audio that actually
    /// produced it — within ±0.2 s at both edges.
    ///
    /// This measures the retention path's own clock: capture chunking, sample
    /// indexing and second↔index rounding. It holds the buffer to the tolerance
    /// W2-A needs. It does NOT measure whether SFSpeech's reported `end_ts`
    /// agrees with the session clock — that is an engine-side question the
    /// bridge probe answers, and it is stated as such in the report.
    #[test]
    fn utterance_end_ts_maps_to_its_audio_window_within_tolerance() {
        /// F3 go/no-go edge tolerance (seconds) for end_ts → PCM mapping.
        const TOLERANCE: f32 = 0.2;
        let spans = [(0.5f32, 2.0f32), (2.5, 4.0), (4.4, 5.6)];
        let session = fixture_session(6.0, &spans);

        let mut buffer = LiveAudioBuffer::new(RATE, DEFAULT_RETENTION_SECS);
        push_chunked(&mut buffer, &session);

        // Apple seals utterances at the end of each span; the consumer resolves
        // each one against the previous boundary, exactly as W2-A will.
        let mut prev_end = 0.0f32;
        for (i, &(speech_start, speech_end)) in spans.iter().enumerate() {
            let end_ts = speech_end;
            let window = buffer.window(prev_end, end_ts).unwrap_or_else(|| {
                panic!("utterance {i}: window({prev_end}, {end_ts}) unresolved")
            });

            let (rel_start, rel_end) = speech_edges_secs(&window)
                .unwrap_or_else(|| panic!("utterance {i}: silent window"));
            let measured_start = prev_end + rel_start;
            let measured_end = prev_end + rel_end;

            let start_drift = (measured_start - speech_start).abs();
            let end_drift = (measured_end - speech_end).abs();
            // Printed so the F3 go/no-go carries numbers, not just a green tick
            // (`cargo test ... -- --nocapture`).
            println!(
                "F3 utterance {i}: window({prev_end:.3}, {end_ts:.3}) \
                 start expected {speech_start:.3}s measured {measured_start:.3}s drift {start_drift:.4}s | \
                 end expected {speech_end:.3}s measured {measured_end:.3}s drift {end_drift:.4}s"
            );
            assert!(
                start_drift <= TOLERANCE,
                "utterance {i}: start drift {start_drift:.4}s > {TOLERANCE}s \
                 (expected {speech_start:.3}s, measured {measured_start:.3}s)"
            );
            assert!(
                end_drift <= TOLERANCE,
                "utterance {i}: end drift {end_drift:.4}s > {TOLERANCE}s \
                 (expected {speech_end:.3}s, measured {measured_end:.3}s)"
            );

            prev_end = end_ts;
        }
    }
}
