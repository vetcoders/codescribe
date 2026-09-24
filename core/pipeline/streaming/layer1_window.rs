//! Layer 1 window: coalesce ~5 Apple segments into one Whisper job.
//!
//! Apple seals short fragments. Diffing each fragment against its own Whisper
//! window hits the change-ratio cap and leaves the chopped canvas standing.
//! This module joins a handful of those fragments — text, PCM, and char
//! offsets — so one decode can cover the whole sentence. It builds windows
//! only: every member occurrence keeps its own PCM identity, and the
//! returned candidate is admitted per occurrence by the acoustic ledger.

use std::time::{Duration, Instant};

use crate::pipeline::acoustic_ledger::OccurrenceIdentity;

/// One sealed Apple fragment waiting to share a Whisper window.
#[derive(Debug, Clone)]
pub struct CoalescedPiece {
    pub utterance_id: u64,
    /// Exact physical occurrence whose launched Whisper slot this piece owns.
    pub occurrence: OccurrenceIdentity,
    pub committed_text: String,
    pub audio: Vec<f32>,
    pub sample_start: u64,
    pub sample_end: u64,
    pub start_ts: f32,
    pub covered_through_secs: f32,
    pub segment_count: usize,
}

/// Ready-to-send Layer 1 job built from one or more coalesced pieces.
#[derive(Debug, Clone)]
pub struct CoalesceFlush {
    pub committed_text: String,
    pub audio: Vec<f32>,
    pub member_ids: Vec<(u64, f32)>,
    /// Exact identities survive pending-presentation removal and queue loss.
    pub member_occurrences: Vec<(u64, OccurrenceIdentity)>,
    pub neighbour_context: String,
    pub sample_start: u64,
    pub sample_end: u64,
    /// Samples inside `[sample_start, sample_end)` whose pins may be admitted.
    /// A leading overlap prefix is decoder context and stays outside this range.
    pub admit_sample_start: u64,
    pub admit_sample_end: u64,
    pub primary_utterance_id: u64,
}

/// Rolling buffer of sealed Apple fragments for one Layer 1 decode.
#[derive(Debug, Default)]
pub struct Layer1Coalesce {
    pieces: Vec<CoalescedPiece>,
    neighbour_before: String,
    segments: usize,
    deadline: Option<Instant>,
    sample_rate: u32,
    /// Suffix of the last emitted contiguous run, replayed only when the next
    /// piece continues that exact capture clock.
    overlap_tail: Option<OverlapTail>,
}

#[derive(Debug, Clone)]
struct OverlapTail {
    session: String,
    capture_epoch: u64,
    sample_end: u64,
    audio: Vec<f32>,
}

impl Layer1Coalesce {
    /// Darek's live window: swap after about five Apple segments.
    pub const TARGET_SEGMENTS: usize = 5;
    /// Target ceiling for coalescing distinct closed occurrences. A single
    /// longer occurrence still needs its own PCM-bounded observation policy.
    pub const MAX_AUDIO_SECS: f32 = 4.0;
    /// Speech-proven PCM repeated at the start of the next observation.
    pub const OVERLAP_SECS: f32 = 1.0;
    /// A pause this long is a sentence boundary — flush what we have.
    pub const PAUSE_SECS: f32 = 1.2;

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// Remember the canvas already sealed before the next piece.
    pub fn set_neighbour(&mut self, neighbour: impl Into<String>) {
        if self.pieces.is_empty() {
            self.neighbour_before = neighbour.into();
        }
    }

    /// Push a sealed fragment. Returns a flush when the window is full, or
    /// when `piece` starts after a sentence pause (the previous window first).
    #[cfg(test)]
    pub fn push(&mut self, piece: CoalescedPiece, sample_rate: u32) -> Vec<CoalesceFlush> {
        self.push_at(piece, sample_rate, Instant::now())
    }

    pub fn push_at(
        &mut self,
        piece: CoalescedPiece,
        sample_rate: u32,
        now: Instant,
    ) -> Vec<CoalesceFlush> {
        self.sample_rate = sample_rate.max(1);
        let mut out = self.flush_due(now);
        if let Some(last) = self.pieces.last() {
            let gap = piece.start_ts - last.covered_through_secs;
            if gap >= Self::PAUSE_SECS {
                out.extend(self.take_flushes());
            }
        }
        let max_samples = window_samples(self.sample_rate);
        let piece_samples = piece.sample_end.saturating_sub(piece.sample_start);
        if piece_samples > max_samples {
            out.extend(self.take_flushes());
            out.extend(self.emit_long_piece(piece));
            return out;
        }
        let held_samples = self.pieces.iter().fold(0_u64, |total, held| {
            total.saturating_add(held.sample_end.saturating_sub(held.sample_start))
        });
        if !self.pieces.is_empty() && held_samples.saturating_add(piece_samples) > max_samples {
            out.extend(self.take_flushes());
        }
        if self.pieces.is_empty() && self.neighbour_before.is_empty() {
            // Neighbour is set by the caller before the first push of a window.
        }
        if self.pieces.is_empty() {
            self.deadline = Some(now + Duration::from_millis(1_200));
        }
        self.segments = self.segments.saturating_add(piece.segment_count.max(1));
        self.pieces.push(piece);
        if self.should_flush(sample_rate) {
            out.extend(self.take_flushes());
        }
        out
    }

    /// Drain whatever is held — session end, epoch sleep, or test.
    ///
    /// Returns one flush per contiguous PCM run, so a held window with a gap in
    /// it drains as several admissible requests rather than one that lies about
    /// its range.
    pub fn force_flush(&mut self) -> Vec<CoalesceFlush> {
        self.take_flushes()
    }

    /// Oldest closed member owns the deadline. More pieces cannot postpone it.
    pub fn flush_due(&mut self, now: Instant) -> Vec<CoalesceFlush> {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.take_flushes()
        } else {
            Vec::new()
        }
    }

    fn should_flush(&self, sample_rate: u32) -> bool {
        if self.pieces.is_empty() {
            return false;
        }
        if self.segments >= Self::TARGET_SEGMENTS {
            return true;
        }
        let samples: u64 = self
            .pieces
            .iter()
            .map(|p| p.sample_end.saturating_sub(p.sample_start))
            .sum();
        let rate = sample_rate.max(1) as f32;
        (samples as f32 / rate) >= Self::MAX_AUDIO_SECS
    }

    fn take_flushes(&mut self) -> Vec<CoalesceFlush> {
        if self.pieces.is_empty() {
            return Vec::new();
        }
        let pieces = std::mem::take(&mut self.pieces);
        self.segments = 0;
        self.deadline = None;
        let neighbour_context = std::mem::take(&mut self.neighbour_before);
        let prefix = self.overlap_tail.take();
        let flushes = build_flushes(pieces, neighbour_context, prefix);
        self.remember_overlap(flushes.last());
        flushes
    }

    /// One occurrence longer than the observation budget becomes several
    /// 4 s windows stepped by 3 s. Exclusive admit ranges partition the
    /// occurrence; the shared second is PCM context, not a second member.
    fn emit_long_piece(&mut self, piece: CoalescedPiece) -> Vec<CoalesceFlush> {
        let max_samples = window_samples(self.sample_rate);
        let overlap = overlap_samples(self.sample_rate).min(max_samples.saturating_sub(1));
        let step = max_samples.saturating_sub(overlap).max(1);
        let mut cursor = piece.sample_start;
        let mut flushes = Vec::new();
        let mut prefix = self
            .overlap_tail
            .take()
            .filter(|tail| tail.continues(&piece));
        while cursor < piece.sample_end {
            let window_end = cursor.saturating_add(max_samples).min(piece.sample_end);
            let last = window_end == piece.sample_end;
            let admit_end = if last {
                window_end
            } else {
                cursor.saturating_add(step).min(window_end)
            };
            let local_start = cursor.saturating_sub(piece.sample_start) as usize;
            let local_end = window_end.saturating_sub(piece.sample_start) as usize;
            let slice = piece
                .audio
                .get(local_start..local_end)
                .unwrap_or_default()
                .to_vec();
            let (sample_start, audio) = match prefix.take() {
                Some(tail) if tail.sample_end == cursor => {
                    let start = tail.sample_end.saturating_sub(tail.audio.len() as u64);
                    let mut audio = tail.audio;
                    audio.extend_from_slice(&slice);
                    (start, audio)
                }
                _ => (cursor, slice),
            };
            flushes.push(flush_from_piece(
                &piece,
                audio,
                sample_start,
                window_end,
                cursor,
                admit_end,
            ));
            cursor = admit_end;
        }
        self.remember_overlap(flushes.last());
        flushes
    }

    fn remember_overlap(&mut self, flush: Option<&CoalesceFlush>) {
        let Some(flush) = flush else {
            return;
        };
        let Some((_, member)) = flush.member_occurrences.last() else {
            return;
        };
        let keep = overlap_samples(self.sample_rate).min(flush.audio.len() as u64) as usize;
        if keep == 0 {
            self.overlap_tail = None;
            return;
        }
        let start = flush.audio.len() - keep;
        self.overlap_tail = Some(OverlapTail {
            session: member.session.clone(),
            capture_epoch: member.capture_epoch,
            sample_end: flush.sample_end,
            audio: flush.audio[start..].to_vec(),
        });
    }
}

impl OverlapTail {
    fn continues(&self, piece: &CoalescedPiece) -> bool {
        self.session == piece.occurrence.session
            && self.capture_epoch == piece.occurrence.capture_epoch
            && self.sample_end == piece.sample_start
            && !self.audio.is_empty()
    }
}

fn window_samples(sample_rate: u32) -> u64 {
    (Layer1Coalesce::MAX_AUDIO_SECS * sample_rate.max(1) as f32) as u64
}

fn overlap_samples(sample_rate: u32) -> u64 {
    (Layer1Coalesce::OVERLAP_SECS * sample_rate.max(1) as f32) as u64
}

/// Split a held window into one flush per contiguous PCM run.
///
/// A window used to declare `[first.sample_start, last.sample_end)` while
/// carrying only the concatenated PCM of its pieces. Whenever the pieces were
/// not adjacent — which is the normal case, since the pauses between utterances
/// are not speech and never enter the buffer — the two disagreed, and
/// `TailProviderRequest::validate_pcm` refused the job at the provider seam.
/// Layer 1 then reported a generic provider error for a window that never
/// reached inference. Measured on this module's own five-piece geometry: 70 400
/// samples declared against 31 999 carried.
///
/// Concatenating across the gap would be worse: the joined audio would carry
/// timestamps that mean nothing on the capture clock, and every segment mapped
/// back from it would name samples the operator never spoke. Splitting keeps
/// every request honest — each declares exactly the samples it holds.
fn build_flushes(
    pieces: Vec<CoalescedPiece>,
    neighbour_context: String,
    prefix: Option<OverlapTail>,
) -> Vec<CoalesceFlush> {
    let mut runs: Vec<Vec<CoalescedPiece>> = Vec::new();
    for piece in pieces {
        match runs.last_mut() {
            Some(run)
                if run.last().is_some_and(|previous| {
                    previous.sample_end == piece.sample_start
                        && previous.occurrence.same_capture(&piece.occurrence)
                }) =>
            {
                run.push(piece);
            }
            _ => runs.push(vec![piece]),
        }
    }
    runs.into_iter()
        .enumerate()
        .map(|(index, run)| {
            // Only the first run inherits the left neighbour; the runs after it
            // are preceded by their own predecessor inside this window.
            let context = if index == 0 {
                neighbour_context.clone()
            } else {
                String::new()
            };
            let prefix = (index == 0)
                .then_some(prefix.clone())
                .flatten()
                .filter(|tail| run.first().is_some_and(|piece| tail.continues(piece)));
            build_flush(run, context, prefix)
        })
        .collect()
}

fn build_flush(
    pieces: Vec<CoalescedPiece>,
    neighbour_context: String,
    prefix: Option<OverlapTail>,
) -> CoalesceFlush {
    let mut committed_text = String::new();
    let mut audio = Vec::new();
    let mut member_ids = Vec::with_capacity(pieces.len());
    let mut member_occurrences = Vec::with_capacity(pieces.len());
    let admit_sample_start = pieces.first().map_or(0, |p| p.sample_start);
    let sample_end = pieces.last().map_or(0, |p| p.sample_end);
    let primary_utterance_id = pieces.last().map_or(0, |p| p.utterance_id);
    let sample_start = prefix.as_ref().map_or(admit_sample_start, |tail| {
        tail.sample_end.saturating_sub(tail.audio.len() as u64)
    });
    if let Some(tail) = prefix {
        audio.extend_from_slice(&tail.audio);
    }
    for piece in pieces {
        if !piece.committed_text.is_empty() {
            if !committed_text.is_empty() {
                committed_text.push(' ');
            }
            committed_text.push_str(&piece.committed_text);
        }
        member_ids.push((piece.utterance_id, piece.covered_through_secs));
        member_occurrences.push((piece.utterance_id, piece.occurrence.clone()));
        debug_assert_eq!(
            piece.audio.len() as u64,
            piece.sample_end.saturating_sub(piece.sample_start),
            "a piece must carry the PCM range it declares before it can be coalesced"
        );
        // Runs are split at every gap and capture boundary. Never repair an
        // invalid payload by padding or truncating: provider validation refuses it.
        audio.extend_from_slice(&piece.audio);
    }
    CoalesceFlush {
        committed_text,
        audio,
        member_ids,
        member_occurrences,
        neighbour_context,
        sample_start,
        sample_end,
        admit_sample_start,
        admit_sample_end: sample_end,
        primary_utterance_id,
    }
}

fn flush_from_piece(
    piece: &CoalescedPiece,
    audio: Vec<f32>,
    sample_start: u64,
    sample_end: u64,
    admit_sample_start: u64,
    admit_sample_end: u64,
) -> CoalesceFlush {
    CoalesceFlush {
        committed_text: piece.committed_text.clone(),
        audio,
        member_ids: vec![(piece.utterance_id, piece.covered_through_secs)],
        member_occurrences: vec![(piece.utterance_id, piece.occurrence.clone())],
        neighbour_context: String::new(),
        sample_start,
        sample_end,
        admit_sample_start,
        admit_sample_end,
        primary_utterance_id: piece.utterance_id,
    }
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
