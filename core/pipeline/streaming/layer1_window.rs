//! The session's single work plan on the original capture sample clock.
//!
//! Planning owns coordinates, never PCM, occurrence identity, labels or seals.
//! Every accepted window is consumed once, including failed provider work.

use crate::stt::tail_provider::TailSampleRange;

#[derive(Debug)]
pub(crate) struct CaptureWindowPlan {
    session: String,
    capture_epoch: u64,
    window_samples: u64,
    step_samples: u64,
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
            eof: None,
            next_start: 0,
            last_full_end: 0,
            offered: None,
            finished: sample_rate == 0,
        }
    }

    /// Peeking never spends work. Backpressure retains the exact offered frame.
    /// The first EOF freezes the capture head while earlier windows drain.
    /// A smaller head withholds unavailable work without revoking its offer.
    pub(crate) fn next_due(&mut self, capture_end: u64, stopping: bool) -> Option<TailSampleRange> {
        if self.finished {
            return None;
        }
        if stopping {
            self.eof.get_or_insert(capture_end);
        }
        let capture_limit = self.eof.unwrap_or(capture_end);
        if let Some(offered) = &self.offered {
            return (offered.sample_end <= capture_end.min(capture_limit)).then(|| offered.clone());
        }
        let end = match self.next_start.checked_add(self.window_samples) {
            Some(full_end) if full_end <= capture_limit => full_end,
            _ if self.eof.is_some() && capture_limit > self.last_full_end => capture_limit,
            _ => {
                if self.eof.is_some() {
                    self.finished = true;
                }
                return None;
            }
        };
        if self.next_start >= end {
            self.finished = self.eof.is_some();
            return None;
        }
        if end > capture_end {
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
            self.finished = self.eof.is_some_and(|end| end <= self.last_full_end);
        } else {
            self.finished = true;
        }
        true
    }

    /// No unaccounted future frame starts before this sample; `u64::MAX` means
    /// the plan is exhausted. Accepted jobs must still return before the session
    /// may close an owner's horizon.
    pub(crate) fn admission_horizon(&self) -> u64 {
        if self.finished {
            u64::MAX
        } else {
            self.next_start
        }
    }

    /// No further ranges can be offered, regardless of accepted jobs in flight.
    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }
}

#[cfg(test)]
impl Layer1Coalesce {
    /// Push a sealed fragment. Returns a flush when the window is full, or
    /// when `piece` starts after a sentence pause (the previous window first).
    #[cfg(test)]
    pub fn push(&mut self, piece: CoalescedPiece, sample_rate: u32) -> Vec<CoalesceFlush> {
        self.push_at(piece, sample_rate, Instant::now())
    }
}

#[cfg(test)]
fn window_samples(sample_rate: u32) -> u64 {
    (Layer1Coalesce::MAX_AUDIO_SECS * sample_rate.max(1) as f32) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(id: u64, text: &str, start_ts: f32, end_ts: f32, segs: usize) -> CoalescedPiece {
        let rate = 16_000u64;
        CoalescedPiece {
            utterance_id: id,
            occurrence: OccurrenceIdentity::new(
                "layer1-window-test",
                1,
                (start_ts * rate as f32) as u64,
                (end_ts * rate as f32) as u64,
            ),
            committed_text: text.to_string(),
            // Production builds a piece from a resolved audio window, so the
            // carried PCM always equals the declared range. Deriving the length
            // from the range keeps the fixture honest about that relationship
            // instead of re-rounding the seconds a second time.
            audio: vec![
                0.0;
                ((end_ts * rate as f32) as u64 - (start_ts * rate as f32) as u64) as usize
            ],
            sample_start: (start_ts * rate as f32) as u64,
            sample_end: (end_ts * rate as f32) as u64,
            start_ts,
            covered_through_secs: end_ts,
            segment_count: segs,
        }
    }

    #[test]
    fn oldest_deadline_preserves_context_without_waiting_for_the_next_piece() {
        let now = Instant::now();
        let mut buffer = Layer1Coalesce::default();
        buffer.set_neighbour("previous");
        assert!(
            buffer
                .push_at(piece(1, "one", 0.0, 0.4, 1), 16_000, now)
                .is_empty()
        );
        assert!(
            buffer
                .push_at(
                    piece(2, "two", 0.4, 0.8, 1),
                    16_000,
                    now + Duration::from_millis(1_000)
                )
                .is_empty()
        );
        let flushes = buffer.flush_due(now + Duration::from_millis(1_200));
        assert_eq!(flushes.len(), 1);
        assert_eq!(flushes[0].member_occurrences.len(), 2);
        assert_eq!(flushes[0].committed_text, "one two");
        assert_eq!(flushes[0].neighbour_context, "previous");
        assert!(buffer.flush_due(now + Duration::from_secs(20)).is_empty());
    }

    #[test]
    fn adjacent_samples_from_different_epochs_never_share_a_request() {
        let now = Instant::now();
        let mut buffer = Layer1Coalesce::default();
        let first = piece(1, "", 0.0, 0.4, 1);
        let mut second = piece(2, "", 0.4, 0.8, 1);
        second.occurrence.capture_epoch = 2;
        assert!(buffer.push_at(first, 16_000, now).is_empty());
        assert!(buffer.push_at(second, 16_000, now).is_empty());
        let flushes = buffer.force_flush();
        assert_eq!(flushes.len(), 2);
        assert!(flushes.iter().all(|flush| flush.committed_text.is_empty()));
        assert_eq!(flushes[0].member_occurrences[0].1.capture_epoch, 1);
        assert_eq!(flushes[1].member_occurrences[0].1.capture_epoch, 2);
        for flush in flushes {
            assert_eq!(
                flush.audio.len() as u64,
                flush.sample_end - flush.sample_start
            );
        }
    }

    #[test]
    fn flushes_after_five_segments() {
        let mut buf = Layer1Coalesce::default();
        buf.set_neighbour("already sealed");
        let mut flushes = Vec::new();
        // Adjacent pieces: one window, one contiguous PCM run.
        for i in 0..5 {
            flushes.extend(buf.push(
                piece(i + 1, "słowo", i as f32 * 0.4, (i + 1) as f32 * 0.4, 1),
                16_000,
            ));
        }
        assert_eq!(flushes.len(), 1);
        assert_eq!(flushes[0].member_occurrences.len(), 5);
        assert_eq!(flushes[0].committed_text, "słowo słowo słowo słowo słowo");
        assert_eq!(flushes[0].neighbour_context, "already sealed");
        assert_eq!(flushes[0].primary_utterance_id, 5);
        assert!(buf.is_empty());
    }

    /// Non-adjacent pieces drain as one flush per contiguous run.
    ///
    /// Coalescing five gapped utterances into a single request meant declaring
    /// `[first.start, last.end)` over audio the window never held — the pauses
    /// between utterances are not speech and never enter the buffer. Each run
    /// now declares exactly the samples it carries.
    #[test]
    fn a_gap_between_pieces_splits_the_window_instead_of_faking_its_range() {
        let mut buf = Layer1Coalesce::default();
        buf.set_neighbour("already sealed");
        let mut flushes = Vec::new();
        for i in 0..5 {
            flushes.extend(buf.push(piece(i + 1, "słowo", i as f32, i as f32 + 0.4, 1), 16_000));
        }
        assert_eq!(flushes.len(), 5, "one flush per contiguous PCM run");
        for flush in &flushes {
            assert_eq!(
                flush.sample_end - flush.sample_start,
                flush.audio.len() as u64,
                "a window may not promise audio it dropped"
            );
        }
        assert_eq!(
            flushes[0].neighbour_context, "already sealed",
            "only the first run inherits the left neighbour"
        );
        assert_eq!(flushes[1].neighbour_context, "");
        assert!(buf.is_empty());
    }

    #[test]
    fn pause_flushes_the_previous_window() {
        let mut buf = Layer1Coalesce::default();
        assert!(buf.push(piece(1, "raz", 0.0, 0.5, 1), 16_000).is_empty());
        let flushes = buf.push(piece(2, "dwa", 3.0, 3.4, 1), 16_000);
        assert_eq!(flushes.len(), 1);
        assert_eq!(flushes[0].member_occurrences.len(), 1);
        assert_eq!(flushes[0].committed_text, "raz");
        assert!(!buf.is_empty());
    }

    #[test]
    fn coalescing_flushes_before_a_four_second_window_would_be_exceeded() {
        let mut buf = Layer1Coalesce::default();
        let now = Instant::now();
        assert!(
            buf.push_at(piece(1, "one", 0.0, 1.5, 1), 16_000, now)
                .is_empty()
        );
        assert!(
            buf.push_at(piece(2, "two", 1.5, 3.0, 1), 16_000, now)
                .is_empty()
        );
        let flushes = buf.push_at(piece(3, "three", 3.0, 4.5, 1), 16_000, now);
        assert_eq!(flushes.len(), 1);
        assert_eq!(flushes[0].sample_start, 0);
        assert_eq!(flushes[0].sample_end, 48_000);
        assert_eq!(flushes[0].audio.len(), 48_000);
        assert_eq!(flushes[0].member_occurrences.len(), 2);
        let remaining = buf.force_flush();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].sample_start, 32_000);
        assert_eq!(remaining[0].sample_end, 72_000);
        assert_eq!(remaining[0].audio.len(), 40_000);
        assert_eq!(remaining[0].admit_sample_start, 48_000);
        assert_eq!(remaining[0].admit_sample_end, 72_000);
        assert_eq!(remaining[0].member_occurrences.len(), 1);
        assert_eq!(remaining[0].member_occurrences[0].1.sample_start, 48_000);
    }

    #[test]
    fn a_long_occurrence_becomes_four_second_windows_with_one_second_overlap() {
        let mut buf = Layer1Coalesce::default();
        let now = Instant::now();
        let flushes = buf.push_at(piece(1, "long", 0.0, 10.0, 1), 16_000, now);
        assert_eq!(flushes.len(), 3);
        let windows = [(0, 64_000), (48_000, 112_000), (96_000, 160_000)];
        let admit = [(0, 48_000), (48_000, 96_000), (96_000, 160_000)];
        for (index, flush) in flushes.iter().enumerate() {
            assert_eq!(flush.sample_start, windows[index].0);
            assert_eq!(flush.sample_end, windows[index].1);
            assert_eq!(
                flush.audio.len() as u64,
                flush.sample_end - flush.sample_start
            );
            assert_eq!(flush.admit_sample_start, admit[index].0);
            assert_eq!(flush.admit_sample_end, admit[index].1);
            assert_eq!(flush.member_occurrences.len(), 1);
            assert_eq!(flush.member_occurrences[0].1.sample_start, 0);
            assert_eq!(flush.member_occurrences[0].1.sample_end, 160_000);
            assert_eq!(flush.primary_utterance_id, 1);
        }
        let gaps: u64 = flushes
            .windows(2)
            .map(|pair| pair[1].admit_sample_start - pair[0].admit_sample_end)
            .sum();
        assert_eq!(gaps, 0, "exclusive ranges partition the occurrence");
    }

    #[test]
    fn overlap_does_not_cross_a_silence_gap_or_a_capture_boundary() {
        let mut buf = Layer1Coalesce::default();
        let now = Instant::now();
        let first = buf.push_at(piece(1, "one", 0.0, 2.0, 1), 16_000, now);
        assert!(first.is_empty());
        let paused = buf.push_at(piece(2, "two", 4.0, 5.0, 1), 16_000, now);
        assert_eq!(paused.len(), 1);
        assert_eq!(paused[0].sample_start, 0);
        assert_eq!(paused[0].audio.len(), 32_000);
        let held = buf.force_flush();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].sample_start, 64_000);
        assert_eq!(
            held[0].audio.len(),
            16_000,
            "the gap is not filled with PCM"
        );

        let mut buf = Layer1Coalesce::default();
        assert!(
            buf.push_at(piece(1, "one", 0.0, 2.0, 1), 16_000, now)
                .is_empty()
        );
        let mut other_epoch = piece(2, "two", 2.0, 3.0, 1);
        other_epoch.occurrence.capture_epoch = 2;
        other_epoch.sample_start = 32_000;
        other_epoch.sample_end = 48_000;
        other_epoch.audio = vec![0.0; 16_000];
        let flushes = buf.push_at(other_epoch, 16_000, now);
        assert!(flushes.is_empty());
        let drained = buf.force_flush();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].audio.len(), 32_000);
        assert_eq!(drained[1].audio.len(), 16_000);
        assert_eq!(drained[1].member_occurrences[0].1.capture_epoch, 2);
    }

    #[test]
    fn blank_apple_text_still_carries_speech_pcm_for_one_occurrence() {
        let mut buf = Layer1Coalesce::default();
        let now = Instant::now();
        assert!(
            buf.push_at(piece(7, "", 0.0, 1.5, 1), 16_000, now)
                .is_empty()
        );
        let flushes = buf.force_flush();
        assert_eq!(flushes.len(), 1);
        assert!(flushes[0].committed_text.is_empty());
        assert_eq!(flushes[0].audio.len(), 24_000);
        assert_eq!(flushes[0].member_occurrences.len(), 1);
        assert_eq!(flushes[0].member_occurrences[0].0, 7);
        assert_eq!(flushes[0].admit_sample_start, flushes[0].sample_start);
        assert_eq!(flushes[0].admit_sample_end, flushes[0].sample_end);
    }

    fn drive(pieces: Vec<CoalescedPiece>) -> Vec<CoalesceFlush> {
        let mut buf = Layer1Coalesce::default();
        let now = Instant::now();
        let mut flushes = Vec::new();
        for piece in pieces {
            flushes.extend(buf.push_at(piece, 16_000, now));
        }
        flushes.extend(buf.force_flush());
        flushes
    }

    /// Contract step 1–2: a window is at most 4 s of contiguous PCM, the next
    /// window shares ~1 s with it, and exclusive admits partition the speech.
    fn assert_four_second_cadence(flushes: &[CoalesceFlush], end: u64) {
        let max = window_samples(16_000);
        let overlap = overlap_samples(16_000);
        assert!(!flushes.is_empty(), "the pieces must produce requests");
        for flush in flushes {
            let declared = flush.sample_end.saturating_sub(flush.sample_start);
            assert_eq!(
                flush.audio.len() as u64,
                declared,
                "a request must carry the PCM range it declares"
            );
            assert!(
                declared <= max,
                "request [{}, {}) carries {declared} samples; the budget is {max}",
                flush.sample_start,
                flush.sample_end
            );
            assert!(flush.admit_sample_start >= flush.sample_start);
            assert!(flush.admit_sample_end <= flush.sample_end);
            assert!(flush.admit_sample_end > flush.admit_sample_start);
        }
        for pair in flushes.windows(2) {
            let start = pair[0].sample_start.max(pair[1].sample_start);
            let shared = pair[0]
                .sample_end
                .min(pair[1].sample_end)
                .saturating_sub(start);
            assert_eq!(
                shared, overlap,
                "consecutive windows [{}, {}) and [{}, {}) must share ~1 s",
                pair[0].sample_start, pair[0].sample_end, pair[1].sample_start, pair[1].sample_end
            );
        }
        let mut cursor = 0_u64;
        for flush in flushes {
            assert_eq!(
                flush.admit_sample_start, cursor,
                "exclusive admit ranges abut"
            );
            cursor = flush.admit_sample_end;
        }
        assert_eq!(cursor, end, "exclusive admit ranges partition the speech");
    }

    fn assert_member_occurrences_are_unsplit(
        flushes: &[CoalesceFlush],
        originals: &[OccurrenceIdentity],
    ) {
        for original in originals {
            let mut spans = Vec::new();
            for flush in flushes {
                let Some((_, member)) = flush
                    .member_occurrences
                    .iter()
                    .find(|(_, occurrence)| occurrence == original)
                else {
                    continue;
                };
                assert_eq!(member.sample_start, original.sample_start);
                assert_eq!(member.sample_end, original.sample_end);
                assert_eq!(member.capture_epoch, original.capture_epoch);
                assert_eq!(member.session, original.session);
                let start = flush.admit_sample_start.max(original.sample_start);
                let end = flush.admit_sample_end.min(original.sample_end);
                if end > start {
                    spans.push((start, end));
                }
            }
            assert!(
                !spans.is_empty(),
                "occurrence [{}, {}) was never admitted",
                original.sample_start,
                original.sample_end
            );
            spans.sort_unstable();
            assert_eq!(spans[0].0, original.sample_start);
            let mut cursor = original.sample_start;
            for (start, end) in spans {
                assert_eq!(
                    start, cursor,
                    "admits inside [{}, {}) must abut",
                    original.sample_start, original.sample_end
                );
                cursor = end;
            }
            assert_eq!(
                cursor, original.sample_end,
                "admits must partition occurrence [{}, {})",
                original.sample_start, original.sample_end
            );
        }
    }

    #[test]
    fn abutting_four_second_pieces_count_the_overlap_prefix_inside_the_budget() {
        let pieces = vec![piece(1, "one", 0.0, 4.0, 1), piece(2, "two", 4.0, 8.0, 1)];
        let originals: Vec<_> = pieces
            .iter()
            .map(|piece| piece.occurrence.clone())
            .collect();
        let flushes = drive(pieces);
        assert_four_second_cadence(&flushes, 128_000);
        assert_member_occurrences_are_unsplit(&flushes, &originals);
    }

    #[test]
    fn abutting_mixed_pieces_count_the_overlap_prefix_inside_the_budget() {
        let pieces = vec![
            piece(1, "one", 0.0, 1.5, 1),
            piece(2, "two", 1.5, 4.5, 1),
            piece(3, "three", 4.5, 8.0, 1),
        ];
        let originals: Vec<_> = pieces
            .iter()
            .map(|piece| piece.occurrence.clone())
            .collect();
        let flushes = drive(pieces);
        assert_four_second_cadence(&flushes, 128_000);
        assert_member_occurrences_are_unsplit(&flushes, &originals);
    }
}

/// Parked conservation falsifiers for the acoustic-identity cut.
///
/// These encode THE ENGINE contract invariants, not current behaviour. They are
/// `#[ignore]`d because the invariant is not implemented yet — the contract's
/// anti-drift rule requires a temporary OFF to name the falsifier it is waiting
/// for, and this is that falsifier. Un-ignore them in the cut that lands
/// "Acoustic identity cut order" step 3 in `docs/THE_ENGINE_CONTRACT.md`.
#[cfg(test)]
mod ledger_conservation_falsifiers {
    use super::*;
    use crate::stt::tail_provider::{TailProviderRequest, TailRequestIdentity, TailSampleRange};

    fn piece_at(id: u64, text: &str, start_ts: f32, end_ts: f32, segs: usize) -> CoalescedPiece {
        let rate = 16_000u64;
        CoalescedPiece {
            utterance_id: id,
            occurrence: OccurrenceIdentity::new(
                "layer1-window-test",
                1,
                (start_ts * rate as f32) as u64,
                (end_ts * rate as f32) as u64,
            ),
            committed_text: text.to_string(),
            // Production builds a piece from a resolved audio window, so the
            // carried PCM always equals the declared range. Deriving the length
            // from the range keeps the fixture honest about that relationship
            // instead of re-rounding the seconds a second time.
            audio: vec![
                0.0;
                ((end_ts * rate as f32) as u64 - (start_ts * rate as f32) as u64) as usize
            ],
            sample_start: (start_ts * rate as f32) as u64,
            sample_end: (end_ts * rate as f32) as u64,
            start_ts,
            covered_through_secs: end_ts,
            segment_count: segs,
        }
    }

    /// `declare_a_pcm_range_the_payload_does_not_carry`.
    ///
    /// A coalesced window declares `[first.sample_start, last.sample_end)` while
    /// carrying only the concatenated PCM of its pieces. With any gap between
    /// pieces the two disagree, `TailProviderRequest::validate_pcm` refuses the
    /// job, and Layer 1 reports a generic provider error for a window that never
    /// reached inference. Measured on this module's own five-piece geometry:
    /// 70 400 samples declared against 31 999 carried.
    #[test]
    fn coalesced_window_carries_the_pcm_range_it_declares() {
        let mut buf = Layer1Coalesce::default();
        buf.set_neighbour("already sealed");
        let mut flushes = Vec::new();
        for i in 0..5 {
            flushes.extend(buf.push(
                piece_at(i + 1, "słowo", i as f32, i as f32 + 0.4, 1),
                16_000,
            ));
        }
        let flush = &flushes[0];
        let request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: flush.primary_utterance_id,
                range: TailSampleRange {
                    session: "conservation".into(),
                    capture_epoch: 1,
                    sample_start: flush.sample_start,
                    sample_end: flush.sample_end,
                },
            },
            sample_rate: 16_000,
            language: None,
        };
        assert_eq!(
            flush.sample_end - flush.sample_start,
            flush.audio.len() as u64,
            "declared range must equal carried PCM: a window may not promise audio it dropped"
        );
        request
            .validate_pcm(&flush.audio)
            .expect("a coalesced window must be admissible at the provider seam");
    }
}

/// Conservation falsifier: a coalesced window must carry the PCM range it
/// declares, or Layer 1 never reaches inference.
#[cfg(test)]
mod observation_identity_conservation_falsifiers {
    use super::*;
    use crate::stt::tail_provider::{TailProviderRequest, TailRequestIdentity, TailSampleRange};

    fn piece_at(id: u64, text: &str, start_ts: f32, end_ts: f32, segs: usize) -> CoalescedPiece {
        let rate = 16_000u64;
        let sample_start = (start_ts * rate as f32) as u64;
        let sample_end = (end_ts * rate as f32) as u64;
        CoalescedPiece {
            utterance_id: id,
            occurrence: OccurrenceIdentity::new("layer1-window-test", 1, sample_start, sample_end),
            committed_text: text.to_string(),
            audio: vec![0.0; sample_end.saturating_sub(sample_start) as usize],
            sample_start,
            sample_end,
            start_ts,
            covered_through_secs: end_ts,
            segment_count: segs,
        }
    }

    #[test]
    fn coalesced_window_carries_the_pcm_range_it_declares() {
        let mut buf = Layer1Coalesce::default();
        buf.set_neighbour("already sealed");
        let mut flushes = Vec::new();
        for i in 0..5 {
            flushes.extend(buf.push(
                piece_at(i + 1, "słowo", i as f32, i as f32 + 0.4, 1),
                16_000,
            ));
        }
        let flush = &flushes[0];
        let request = TailProviderRequest {
            identity: TailRequestIdentity {
                request_id: flush.primary_utterance_id,
                range: TailSampleRange {
                    session: "conservation".into(),
                    capture_epoch: 1,
                    sample_start: flush.sample_start,
                    sample_end: flush.sample_end,
                },
            },
            sample_rate: 16_000,
            language: None,
        };
        assert_eq!(
            flush.sample_end - flush.sample_start,
            flush.audio.len() as u64,
            "declared range must equal carried PCM: a window may not promise audio it dropped"
        );
        request
            .validate_pcm(&flush.audio)
            .expect("a coalesced window must be admissible at the provider seam");
    }
}
