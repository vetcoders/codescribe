//! Shared W13-0 clock-histogram helpers (test-only).

#[derive(Debug, Clone, serde::Serialize)]
pub struct DurationBucket {
    pub label: &'static str,
    pub count: usize,
}

pub fn duration_buckets(region_lens: &[usize], sample_rate: u32) -> Vec<DurationBucket> {
    let mut buckets = [
        DurationBucket {
            label: "<10ms",
            count: 0,
        },
        DurationBucket {
            label: "10-50ms",
            count: 0,
        },
        DurationBucket {
            label: "50-200ms",
            count: 0,
        },
        DurationBucket {
            label: "200ms-1s",
            count: 0,
        },
        DurationBucket {
            label: ">1s",
            count: 0,
        },
    ];
    let sr = sample_rate.max(1) as f64;
    for &len in region_lens {
        let ms = (len as f64 / sr) * 1000.0;
        let idx = if ms < 10.0 {
            0
        } else if ms < 50.0 {
            1
        } else if ms < 200.0 {
            2
        } else if ms < 1000.0 {
            3
        } else {
            4
        };
        buckets[idx].count += 1;
    }
    buckets.to_vec()
}

/// Apple word-span histogram. Seconds in, because that is what
/// `TranscriptSegment` currently carries (clock lie: f32 seconds).
pub fn histogram_apple_word_spans(segments: &[(f32, f32)]) -> (Vec<DurationBucket>, usize, usize) {
    let mut durations_ms = Vec::new();
    let mut overlap = 0usize;
    let mut restarts = 0usize;
    let mut prev_end = f32::NEG_INFINITY;
    for &(start, end) in segments {
        if !start.is_finite() || !end.is_finite() || end < start {
            continue;
        }
        durations_ms.push(((end - start) * 1000.0).max(0.0) as usize);
        if start + f32::EPSILON < prev_end {
            overlap += 1;
        }
        // Restart: Apple's phrase clock jumped backward by ≥250 ms.
        // `floor()` is useless on sub-second spans (0.7.floor() == 0).
        if prev_end - start >= 0.25 {
            restarts += 1;
        }
        prev_end = prev_end.max(end);
    }
    let histogram = duration_buckets(&durations_ms, 1000);
    (histogram, overlap, restarts)
}
