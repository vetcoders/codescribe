//! Best-effort signal-frame feed for the loopback Voice Lab.
//!
//! The Lab's bound lane consumes Codescribe's own capture instead of opening
//! a second browser microphone, so while a take records, the level worker
//! batches RMS samples and POSTs them to the Lab relay. The feed is strictly
//! best-effort: one failed POST disarms it for the rest of the broadcast
//! session (a machine without a running Lab pays one refused connection,
//! never a stream of them), and nothing here may block or slow the level
//! path — batching is pure, sending is spawned.

use std::time::{Duration, Instant};

/// Where the Lab relay listens. Loopback-only by the Lab's own contract.
pub(crate) const LAB_FRAMES_URL: &str = "http://127.0.0.1:8765/lab/live-frames";

/// One POST at most every this often while samples keep arriving.
const FLUSH_EVERY: Duration = Duration::from_millis(250);
/// The relay admits at most 256 frames per batch; stay well under it.
const FLUSH_AT: usize = 64;

/// Pure batching decision: collect RMS samples, release a batch on time or
/// size. Separated from the HTTP send so the policy is testable without a
/// server.
pub(crate) struct LabFeedBatcher {
    pending: Vec<f32>,
    last_flush: Instant,
}

impl LabFeedBatcher {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            pending: Vec::with_capacity(FLUSH_AT),
            last_flush: now,
        }
    }

    /// Offer one sample; `Some(batch)` means it is time to send.
    pub(crate) fn offer(&mut self, rms: f32, now: Instant) -> Option<Vec<f32>> {
        self.pending.push(rms);
        if self.pending.len() >= FLUSH_AT || now.duration_since(self.last_flush) >= FLUSH_EVERY {
            self.last_flush = now;
            return Some(std::mem::take(&mut self.pending));
        }
        None
    }
}

/// The relay's wire shape for one batch. `peak` is the batch maximum: the
/// level callback publishes block RMS only, and the capture thread is not
/// the place to grow a second metric.
pub(crate) fn frames_payload(batch: &[f32]) -> serde_json::Value {
    let peak = batch.iter().copied().fold(0.0_f32, f32::max);
    serde_json::json!({
        "frames": batch
            .iter()
            .map(|rms| serde_json::json!({ "rms": rms, "peak": peak }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_release_on_time_not_per_sample() {
        let start = Instant::now();
        let mut batcher = LabFeedBatcher::new(start);
        assert!(
            batcher
                .offer(0.1, start + Duration::from_millis(50))
                .is_none()
        );
        assert!(
            batcher
                .offer(0.2, start + Duration::from_millis(100))
                .is_none()
        );
        let batch = batcher
            .offer(0.3, start + Duration::from_millis(260))
            .expect("time flush");
        assert_eq!(batch, vec![0.1, 0.2, 0.3]);
        // The next sample starts a fresh window instead of flushing again.
        assert!(
            batcher
                .offer(0.4, start + Duration::from_millis(270))
                .is_none()
        );
    }

    #[test]
    fn batches_release_on_size_before_time() {
        let start = Instant::now();
        let mut batcher = LabFeedBatcher::new(start);
        for i in 0..63 {
            assert!(batcher.offer(i as f32, start).is_none());
        }
        let batch = batcher.offer(63.0, start).expect("size flush");
        assert_eq!(batch.len(), 64);
    }

    #[test]
    fn payload_carries_every_rms_and_the_batch_peak() {
        let payload = frames_payload(&[0.1, 0.5, 0.2]);
        let frames = payload["frames"].as_array().expect("frames array");
        assert_eq!(frames.len(), 3);
        for frame in frames {
            assert!((frame["peak"].as_f64().expect("peak") - 0.5).abs() < 1e-6);
        }
        assert!((frames[1]["rms"].as_f64().expect("rms") - 0.5).abs() < 1e-6);
    }
}
