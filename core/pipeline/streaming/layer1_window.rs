//! Layer 1 window: coalesce ~5 Apple segments into one Whisper job.
//!
//! Apple seals short fragments. Diffing each fragment against its own Whisper
//! window hits the change-ratio cap and leaves the chopped canvas standing.
//! This module joins a handful of those fragments — text, PCM, and char
//! offsets — so one decode can cover the whole sentence. It builds windows
//! only: every member occurrence keeps its own PCM identity, and the
//! returned candidate is admitted per occurrence by the acoustic ledger.

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
    pub primary_utterance_id: u64,
}

/// Rolling buffer of sealed Apple fragments for one Layer 1 decode.
#[derive(Debug, Default)]
pub struct Layer1Coalesce {
    pieces: Vec<CoalescedPiece>,
    neighbour_before: String,
    segments: usize,
}

impl Layer1Coalesce {
    /// Darek's live window: swap after about five Apple segments.
    pub const TARGET_SEGMENTS: usize = 5;
    /// Hard cap so a long run-on still gets a decode.
    pub const MAX_AUDIO_SECS: f32 = 16.0;
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
    pub fn push(&mut self, piece: CoalescedPiece, sample_rate: u32) -> Vec<CoalesceFlush> {
        let mut out = Vec::new();
        if let Some(last) = self.pieces.last() {
            let gap = piece.start_ts - last.covered_through_secs;
            if gap >= Self::PAUSE_SECS {
                out.extend(self.take_flushes());
            }
        }
        if self.pieces.is_empty() && self.neighbour_before.is_empty() {
            // Neighbour is set by the caller before the first push of a window.
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
        let neighbour_context = std::mem::take(&mut self.neighbour_before);
        build_flushes(pieces, neighbour_context)
    }
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
fn build_flushes(pieces: Vec<CoalescedPiece>, neighbour_context: String) -> Vec<CoalesceFlush> {
    let mut runs: Vec<Vec<CoalescedPiece>> = Vec::new();
    for piece in pieces {
        match runs.last_mut() {
            Some(run)
                if run
                    .last()
                    .is_some_and(|previous| previous.sample_end == piece.sample_start) =>
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
            build_flush(run, context)
        })
        .collect()
}

fn build_flush(pieces: Vec<CoalescedPiece>, neighbour_context: String) -> CoalesceFlush {
    let mut committed_text = String::new();
    let mut audio = Vec::new();
    let mut member_ids = Vec::with_capacity(pieces.len());
    let mut member_occurrences = Vec::with_capacity(pieces.len());
    let sample_start = pieces.first().map_or(0, |p| p.sample_start);
    let sample_end = pieces.last().map_or(0, |p| p.sample_end);
    let primary_utterance_id = pieces.last().map_or(0, |p| p.utterance_id);
    let mut cursor = sample_start;
    for (i, piece) in pieces.into_iter().enumerate() {
        if i > 0 {
            committed_text.push(' ');
        }
        committed_text.push_str(&piece.committed_text);
        member_ids.push((piece.utterance_id, piece.covered_through_secs));
        member_occurrences.push((piece.utterance_id, piece.occurrence.clone()));
        debug_assert_eq!(
            piece.audio.len() as u64,
            piece.sample_end.saturating_sub(piece.sample_start),
            "a piece must carry the PCM range it declares before it can be coalesced"
        );
        let piece_start = piece.sample_start.max(sample_start);
        if piece_start > cursor {
            audio.resize(audio.len() + (piece_start - cursor) as usize, 0.0);
            cursor = piece_start;
        }
        let skip = cursor.saturating_sub(piece_start) as usize;
        if skip < piece.audio.len() {
            audio.extend_from_slice(&piece.audio[skip..]);
            cursor = piece_start + piece.audio.len() as u64;
        }
    }
    let declared = sample_end.saturating_sub(sample_start) as usize;
    if audio.len() < declared {
        audio.resize(declared, 0.0);
    } else if audio.len() > declared {
        audio.truncate(declared);
    }
    CoalesceFlush {
        committed_text,
        audio,
        member_ids,
        member_occurrences,
        neighbour_context,
        sample_start,
        sample_end,
        primary_utterance_id,
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
