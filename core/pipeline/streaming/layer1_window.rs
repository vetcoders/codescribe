//! The session's single work plan on the original capture sample clock.
//!
//! Planning owns coordinates, never PCM, occurrence identity, labels or seals.
//! Every accepted window is consumed once, including failed provider work.

#[cfg(test)]
#[path = "capture_window_plan_contract_tests.rs"]
mod capture_window_plan_contract_tests;

use crate::stt::tail_provider::TailSampleRange;

#[derive(Debug)]
pub(crate) struct CaptureWindowPlan {
    session: String,
    capture_epoch: u64,
    window_samples: u64,
    step_samples: u64,
    capture_end: u64,
    eof: Option<u64>,
    next_start: u64,
    last_full_end: u64,
    offered: Option<TailSampleRange>,
    finished: bool,
}

impl CaptureWindowPlan {
    pub(crate) fn new(session: String, capture_epoch: u64, sample_rate: u32) -> Self {
        Self {
            session,
            capture_epoch,
            window_samples: u64::from(sample_rate) * 9,
            step_samples: u64::from(sample_rate) * 3,
            capture_end: 0,
            eof: None,
            next_start: 0,
            last_full_end: 0,
            offered: None,
            finished: sample_rate == 0,
        }
    }

    /// Peeking never spends work. Backpressure retains the exact offered frame.
    /// The first Stop observation freezes the head. A later, larger sample
    /// count neither hides that offer nor opens another grid past the freeze.
    pub(crate) fn next_due(&mut self, capture_end: u64, stopping: bool) -> Option<TailSampleRange> {
        if self.finished {
            return None;
        }
        // A regressed head hides nothing permanently: the outstanding offer
        // stays put and the next honest head sees it. Stop freezes above.
        if self.eof.is_none() && capture_end < self.capture_end {
            return None;
        }
        self.observe_head(capture_end, stopping);
        if let Some(offered) = &self.offered {
            return Some(offered.clone());
        }
        let head = self.eof.unwrap_or(self.capture_end);
        let Some(full_end) = self.next_start.checked_add(self.window_samples) else {
            if self.eof.is_some() {
                self.finished = true;
            }
            return None;
        };
        let end = if self.window_samples > 0 && full_end <= head {
            full_end
        } else if self.eof.is_some() && head > self.last_full_end && self.next_start < head {
            head
        } else {
            if self.eof.is_some() {
                self.finished = true;
            }
            return None;
        };
        if self.next_start >= end {
            self.finished = self.eof.is_some();
            return None;
        }
        let range = TailSampleRange {
            session: self.session.clone(),
            capture_epoch: self.capture_epoch,
            sample_start: self.next_start,
            sample_end: end,
        };
        self.offered = Some(range.clone());
        Some(range)
    }

    fn observe_head(&mut self, capture_end: u64, stopping: bool) {
        if self.eof.is_some() {
            return;
        }
        if capture_end > self.capture_end {
            self.capture_end = capture_end;
        }
        if stopping {
            self.eof = Some(self.capture_end);
        }
    }

    /// Account only the exact outstanding offer, after transport acceptance or
    /// an explicit unavailable-work receipt. There is no success refund.
    pub(crate) fn account(&mut self, range: &TailSampleRange) -> bool {
        if self.offered.as_ref() != Some(range) {
            return false;
        }
        self.offered = None;
        if range.sample_end - range.sample_start == self.window_samples {
            self.last_full_end = range.sample_end;
            self.next_start = self.next_start.saturating_add(self.step_samples);
            self.finished = self.eof == Some(self.last_full_end);
        } else {
            self.finished = true;
        }
        true
    }

    /// No unaccounted future frame starts before this sample. Accepted jobs
    /// still have to return before the session may close an owner's horizon.
    pub(crate) fn admission_horizon(&self) -> u64 {
        if self.finished {
            u64::MAX
        } else {
            self.next_start
        }
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }
}
