//! The session's single work plan on the original capture sample clock.
//!
//! Planning owns coordinates, never PCM, occurrence identity, labels or seals.
//! Every accepted window is consumed once, including failed provider work.

#[cfg(test)]
#[path = "capture_window_plan_contract_tests.rs"]
mod capture_window_plan_contract_tests;

use crate::stt::tail_provider::TailSampleRange;

/// Length of one Layer 1 Whisper window on the capture clock, in seconds.
///
/// With [`LIVE_STEP_SECS`] every sample is decoded by up to three windows.
/// That overlap is the engine's main second opinion on a word (Apple speaks
/// for a median 16 % of a take's words, div0 15–17 X) and also where most
/// duplicated words come from: 40 % of adjacent identical delivered words
/// share one stretch of PCM.
pub(crate) const LIVE_WINDOW_SECS: u32 = 9;

/// Grid step between Layer 1 window starts, in seconds.
pub(crate) const LIVE_STEP_SECS: u32 = 3;

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
    trial_offer: bool,
    last_trial_bucket: Option<u64>,
}

impl CaptureWindowPlan {
    pub(crate) fn new(session: String, capture_epoch: u64, sample_rate: u32) -> Self {
        Self {
            session,
            capture_epoch,
            window_samples: u64::from(sample_rate) * u64::from(LIVE_WINDOW_SECS),
            step_samples: u64::from(sample_rate) * u64::from(LIVE_STEP_SECS),
            capture_end: 0,
            eof: None,
            next_start: 0,
            last_full_end: 0,
            offered: None,
            finished: sample_rate == 0,
            trial_offer: false,
            last_trial_bucket: None,
        }
    }

    /// Raw decoder chunks own disjoint capture PCM. Without word admission,
    /// overlapping inference text cannot be stitched by lexical deduplication.
    pub(crate) fn set_non_overlapping_publications(&mut self) {
        debug_assert_eq!(self.capture_end, 0);
        debug_assert!(self.offered.is_none());
        self.step_samples = self.window_samples;
    }

    /// Peeking never spends work. Backpressure retains the exact offered frame.
    /// The first Stop observation freezes the head. A later, larger sample
    /// count neither hides that offer nor opens another grid past the freeze.
    pub(crate) fn next_due(&mut self, capture_end: u64, stopping: bool) -> Option<TailSampleRange> {
        if self.trial_offer {
            return self.offered.clone();
        }
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

    /// One additional lexical witness per nine seconds of capture, through the
    /// same offer/account corridor as grid work. Never advances the grid. Stop
    /// may only offer work wholly inside its final nine seconds.
    pub(crate) fn offer_trial(
        &mut self,
        start: u64,
        end: u64,
        capture_end: u64,
        stopping: bool,
    ) -> Option<TailSampleRange> {
        self.observe_head(capture_end, stopping);
        let head = self.eof.unwrap_or(self.capture_end);
        let bucket = head.checked_div(self.window_samples)?;
        if self.offered.is_some()
            || start >= end
            || end > head
            || end - start > self.window_samples
            || self.last_trial_bucket == Some(bucket)
            || (self.eof.is_some() && start < head.saturating_sub(self.window_samples))
        {
            return None;
        }
        let range = TailSampleRange {
            session: self.session.clone(),
            capture_epoch: self.capture_epoch,
            sample_start: start,
            sample_end: end,
        };
        self.last_trial_bucket = Some(bucket);
        self.trial_offer = true;
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
        if self.trial_offer {
            self.trial_offer = false;
            return true;
        }
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
        self.finished && self.offered.is_none()
    }
}
