//! Layer 1 PCM windows: open-speech offers and closed-span coalescing.
//!
//! The open-speech clock can offer measured PCM before Apple finalizes a word.
//! Once physical occurrences close, short Apple fragments can share a Whisper
//! decode. This module builds windows only: the acoustic ledger alone admits
//! returned candidates to the document.

use std::time::{Duration, Instant};

#[cfg(test)]
use crate::audio::capture_receipt::{AcousticSpeechEvidence, CaptureEvidenceIdentity};
use crate::pipeline::acoustic_ledger::OccurrenceIdentity;
#[cfg(test)]
use crate::stt::tail_provider::TailSampleRange;

#[cfg(test)]
use super::silero_fusion::SILERO_RAW_BOUNDARIES_PRODUCER;

/// An immutable L1 request offered while its raw Silero crossing is still open.
///
/// The request may repeat PCM for decoder context, but only the exclusive admit
/// range can later label an occurrence. This is deliberately a capture range,
/// not an `OccurrenceIdentity`: the larger Silero region may still extend.
/// The caller may bind to an already immutable speech subspan while that
/// region remains open, or stage the result until its exact identity is known.
/// It must never mint a provisional end that will be reminted on growth.
///
/// No live-session caller consumes the open-speech clock yet (it arrived with
/// the roman falsifier stack, unwired there too), so it compiles for its
/// falsifiers only until the live session drives it.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSpeechWindow {
    pub request_range: TailSampleRange,
    pub admit_sample_start: u64,
    pub admit_sample_end: u64,
}

/// Offer ~4 s L1 observations from raw Silero crossings, independent of Apple finals.
///
/// One pending offer is retained until the transport acknowledges it. A full
/// queue therefore cannot advance the clock and silently drop the only L1
/// observation of a range. This clock never mints an occurrence or a document
/// mutation; it only describes PCM that the capture owner must resolve exactly.
#[cfg(test)]
#[derive(Debug)]
pub struct OpenSpeechWindowClock {
    capture: CaptureEvidenceIdentity,
    queued_through: Option<u64>,
    pending: Option<OpenSpeechWindow>,
}

#[cfg(test)]
impl OpenSpeechWindowClock {
    pub fn new(capture: CaptureEvidenceIdentity) -> Self {
        Self {
            capture,
            queued_through: None,
            pending: None,
        }
    }

    /// Return the same offer until `ack_queued` confirms transport ownership.
    /// A short open tail waits for more speech; a measured Silero speech-end,
    /// closed island or capture EOF may emit the residual tail without borrowing
    /// PCM from a silent gap. `closed_through` is the capture identity and latest
    /// raw threshold edge from the same Silero ingress that produced `evidence`,
    /// not an Apple final or a window endpoint invented by the caller.
    pub fn next(
        &mut self,
        evidence: &AcousticSpeechEvidence,
        sample_rate: u32,
        closed_through: Option<(CaptureEvidenceIdentity, u64)>,
        capture_closed: bool,
    ) -> Option<OpenSpeechWindow> {
        if evidence.identity() != &self.capture
            || evidence.producer() != SILERO_RAW_BOUNDARIES_PRODUCER
        {
            self.pending = None;
            return None;
        }
        let Some(observed) = evidence.availability().observed_samples() else {
            self.pending = None;
            return None;
        };
        let closed_through = closed_through.and_then(|(identity, end)| {
            (identity == self.capture && end <= observed).then_some(end)
        });
        if let Some(pending) = &self.pending {
            let still_measured = evidence.ranges().iter().any(|range| {
                self.capture.matches(&range.session, range.capture_epoch)
                    && range.sample_start <= pending.request_range.sample_start
                    && pending.request_range.sample_end <= range.sample_end.min(observed)
            });
            if still_measured {
                return Some(pending.clone());
            }
            self.pending = None;
        }

        let max_samples = window_samples(sample_rate);
        let overlap = overlap_samples(sample_rate).min(max_samples.saturating_sub(1));
        for (index, range) in evidence.ranges().iter().enumerate() {
            if !self.capture.matches(&range.session, range.capture_epoch) {
                continue;
            }
            let range_end = range.sample_end.min(observed);
            let admit_start = self
                .queued_through
                .filter(|end| *end > range.sample_start)
                .unwrap_or(range.sample_start);
            if admit_start >= range_end {
                continue;
            }
            let request_start = if self
                .queued_through
                .is_some_and(|end| end > range.sample_start)
            {
                admit_start.saturating_sub(overlap).max(range.sample_start)
            } else {
                range.sample_start
            };
            let full_end = request_start.saturating_add(max_samples);
            let island_closed = capture_closed
                || closed_through.is_some_and(|end| end >= range_end)
                || evidence.ranges()[index + 1..].iter().any(|next| {
                    self.capture.matches(&next.session, next.capture_epoch)
                        && next.sample_start < observed
                        && next.sample_start >= range_end
                });
            let request_end = if full_end <= range_end {
                full_end
            } else if island_closed {
                range_end
            } else {
                continue;
            };
            if request_end <= admit_start {
                continue;
            }
            let offered = OpenSpeechWindow {
                request_range: TailSampleRange {
                    session: self.capture.session.clone(),
                    capture_epoch: self.capture.capture_epoch,
                    sample_start: request_start,
                    sample_end: request_end,
                },
                admit_sample_start: admit_start,
                admit_sample_end: request_end,
            };
            self.pending = Some(offered.clone());
            return Some(offered);
        }
        None
    }

    /// Advance only after the exact offered PCM was accepted by the L1 queue.
    pub fn ack_queued(&mut self, offered: &OpenSpeechWindow) -> bool {
        if self.pending.as_ref() != Some(offered) {
            return false;
        }
        self.queued_through = Some(offered.admit_sample_end);
        self.pending = None;
        true
    }
}

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
    /// Ceiling for one observation, including a retained overlap prefix.
    /// A piece that would push the request past this ceiling is split on the
    /// capture PCM axis; the member occurrence identity stays whole.
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
        if !self.pieces.is_empty() {
            let pending = self
                .prefix_samples_for_held()
                .saturating_add(self.held_samples())
                .saturating_add(piece_samples);
            if pending > max_samples {
                out.extend(self.take_flushes());
            }
        }
        // The prefix is prepended at flush. Count it before accepting the piece,
        // and split on the PCM axis when the piece itself cannot fit.
        let prefix_samples = if self.pieces.is_empty() {
            self.prefix_samples_continuing(&piece)
        } else {
            0
        };
        if piece_samples > max_samples.saturating_sub(prefix_samples) {
            out.extend(self.emit_long_piece(piece));
            return out;
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
        let samples = self
            .held_samples()
            .saturating_add(self.prefix_samples_for_held());
        let rate = sample_rate.max(1) as f32;
        (samples as f32 / rate) >= Self::MAX_AUDIO_SECS
    }

    fn held_samples(&self) -> u64 {
        self.pieces.iter().fold(0_u64, |total, held| {
            total.saturating_add(held.sample_end.saturating_sub(held.sample_start))
        })
    }

    fn prefix_samples_for_held(&self) -> u64 {
        self.pieces
            .first()
            .map(|piece| self.prefix_samples_continuing(piece))
            .unwrap_or(0)
    }

    fn prefix_samples_continuing(&self, piece: &CoalescedPiece) -> u64 {
        self.overlap_tail
            .as_ref()
            .filter(|tail| tail.continues(piece))
            .map(|tail| tail.audio.len() as u64)
            .unwrap_or(0)
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

    /// One occurrence that does not fit in the observation budget becomes
    /// several 4 s windows stepped by 3 s. A retained prefix consumes part of
    /// the first window. Exclusive admit ranges partition the occurrence; the
    /// shared second is PCM context, not a second member.
    fn emit_long_piece(&mut self, piece: CoalescedPiece) -> Vec<CoalesceFlush> {
        let max_samples = window_samples(self.sample_rate);
        let overlap = overlap_samples(self.sample_rate).min(max_samples.saturating_sub(1));
        let mut cursor = piece.sample_start;
        let mut flushes = Vec::new();
        let mut prefix = self
            .overlap_tail
            .take()
            .filter(|tail| tail.continues(&piece));
        while cursor < piece.sample_end {
            let prefix_len = prefix
                .as_ref()
                .filter(|tail| tail.sample_end == cursor && !tail.audio.is_empty())
                .map(|tail| tail.audio.len() as u64)
                .unwrap_or(0);
            let new_budget = max_samples.saturating_sub(prefix_len);
            if new_budget == 0 {
                // The retained prefix already fills the window, so it cannot
                // be prepended without exceeding the budget.
                prefix = None;
                continue;
            }
            let window_end = cursor.saturating_add(new_budget).min(piece.sample_end);
            if window_end <= cursor {
                break;
            }
            let last = window_end == piece.sample_end;
            let stepped = window_end.saturating_sub(overlap);
            let admit_end = if last || stepped <= cursor {
                window_end
            } else {
                stepped
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
    use crate::audio::capture_receipt::AcousticAvailability;

    fn measured_speech(
        session: &str,
        capture_epoch: u64,
        observed_samples: u64,
        ranges: &[(u64, u64)],
    ) -> AcousticSpeechEvidence {
        AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new(session, capture_epoch),
            SILERO_RAW_BOUNDARIES_PRODUCER,
            AcousticAvailability::Observed { observed_samples },
            ranges
                .iter()
                .map(|&(sample_start, sample_end)| TailSampleRange {
                    session: session.to_string(),
                    capture_epoch,
                    sample_start,
                    sample_end,
                })
                .collect(),
        )
    }

    #[test]
    fn open_speech_offers_four_second_windows_without_an_apple_final() {
        let mut clock = OpenSpeechWindowClock::new(CaptureEvidenceIdentity::new("take", 7));
        let five_seconds = measured_speech("take", 7, 5_000, &[(0, 5_000)]);
        let first = clock.next(&five_seconds, 1_000, None, false).unwrap();
        assert_eq!(
            (
                first.request_range.sample_start,
                first.request_range.sample_end
            ),
            (0, 4_000)
        );
        assert_eq!(
            (first.admit_sample_start, first.admit_sample_end),
            (0, 4_000)
        );
        assert_eq!(
            clock.next(&five_seconds, 1_000, None, false),
            Some(first.clone())
        );
        assert!(!clock.ack_queued(&OpenSpeechWindow {
            admit_sample_end: 3_000,
            ..first.clone()
        }));
        assert!(clock.ack_queued(&first));
        assert!(clock.next(&five_seconds, 1_000, None, false).is_none());

        let eight_seconds = measured_speech("take", 7, 8_000, &[(0, 8_000)]);
        let second = clock.next(&eight_seconds, 1_000, None, false).unwrap();
        assert_eq!(
            (
                second.request_range.sample_start,
                second.request_range.sample_end
            ),
            (3_000, 7_000)
        );
        assert_eq!(
            (second.admit_sample_start, second.admit_sample_end),
            (4_000, 7_000)
        );
        assert!(clock.ack_queued(&second));
        assert!(clock.next(&eight_seconds, 1_000, None, false).is_none());

        let last = clock.next(&eight_seconds, 1_000, None, true).unwrap();
        assert_eq!(
            (
                last.request_range.sample_start,
                last.request_range.sample_end
            ),
            (6_000, 8_000)
        );
        assert_eq!(
            (last.admit_sample_start, last.admit_sample_end),
            (7_000, 8_000)
        );
        assert!(clock.ack_queued(&last));
        assert!(clock.next(&eight_seconds, 1_000, None, true).is_none());
    }

    /// A saturated L1 queue is not an acknowledgement. Growth, a real
    /// SpeechEnd, and even the next speech island must not make the clock
    /// skip the first offer or silently grant its PCM to a later request.
    #[test]
    fn full_queue_keeps_open_speech_offer_across_growth_and_close() {
        let capture = CaptureEvidenceIdentity::new("take", 7);
        let mut clock = OpenSpeechWindowClock::new(capture.clone());
        let first_evidence = measured_speech("take", 7, 4_000, &[(0, 4_000)]);
        let first = clock
            .next(&first_evidence, 1_000, None, false)
            .expect("four seconds of open speech must offer L1 before Apple final");
        assert_eq!(
            (
                first.request_range.sample_start,
                first.request_range.sample_end
            ),
            (0, 4_000)
        );

        let grown = measured_speech("take", 7, 7_500, &[(0, 7_500)]);
        assert_eq!(clock.next(&grown, 1_000, None, false), Some(first.clone()));
        let later_island = measured_speech("take", 7, 10_000, &[(0, 7_500), (9_000, 10_000)]);
        let real_close = Some((capture, 7_500));
        assert_eq!(
            clock.next(&later_island, 1_000, real_close.clone(), false),
            Some(first.clone()),
            "queue backpressure must retain the exact unacknowledged PCM offer"
        );

        assert!(clock.ack_queued(&first));
        let second = clock
            .next(&later_island, 1_000, real_close.clone(), false)
            .expect("the retained tail follows only after the first offer is accepted");
        assert_eq!(
            (
                second.request_range.sample_start,
                second.request_range.sample_end
            ),
            (3_000, 7_000)
        );
        assert_eq!(
            (second.admit_sample_start, second.admit_sample_end),
            (4_000, 7_000)
        );
        assert!(clock.ack_queued(&second));
        let tail = clock
            .next(&later_island, 1_000, real_close, false)
            .expect("the measured close releases the last exclusive speech tail");
        assert_eq!(
            (
                tail.request_range.sample_start,
                tail.request_range.sample_end
            ),
            (6_000, 7_500)
        );
        assert_eq!(
            (tail.admit_sample_start, tail.admit_sample_end),
            (7_000, 7_500)
        );
        assert!(clock.ack_queued(&tail));
    }

    #[test]
    fn measured_speech_end_releases_a_short_l1_tail_without_stopping_capture() {
        let mut clock = OpenSpeechWindowClock::new(CaptureEvidenceIdentity::new("take", 7));
        let short_speech = measured_speech("take", 7, 6_000, &[(1_000, 3_000)]);
        assert!(clock.next(&short_speech, 1_000, None, false).is_none());
        assert!(
            clock
                .next(
                    &short_speech,
                    1_000,
                    Some((CaptureEvidenceIdentity::new("take", 7), 6_001)),
                    false,
                )
                .is_none(),
            "a boundary beyond observed PCM is not measured"
        );
        assert!(
            clock
                .next(
                    &short_speech,
                    1_000,
                    Some((CaptureEvidenceIdentity::new("foreign", 7), 3_000)),
                    false,
                )
                .is_none()
        );

        let tail = clock
            .next(
                &short_speech,
                1_000,
                Some((CaptureEvidenceIdentity::new("take", 7), 3_000)),
                false,
            )
            .expect("Silero's real speech-end must release the short final island");
        assert_eq!(
            (
                tail.request_range.sample_start,
                tail.request_range.sample_end
            ),
            (1_000, 3_000)
        );
        assert_eq!(
            (tail.admit_sample_start, tail.admit_sample_end),
            (1_000, 3_000)
        );
        assert!(clock.ack_queued(&tail));
        assert!(
            clock
                .next(
                    &short_speech,
                    1_000,
                    Some((CaptureEvidenceIdentity::new("take", 7), 3_000)),
                    false,
                )
                .is_none()
        );
    }

    #[test]
    fn disjoint_speech_never_borrows_overlap_from_silence() {
        let mut clock = OpenSpeechWindowClock::new(CaptureEvidenceIdentity::new("take", 7));
        let speech = measured_speech("take", 7, 11_000, &[(0, 4_500), (6_000, 11_000)]);
        let mut offers = Vec::new();
        while let Some(window) = clock.next(&speech, 1_000, None, true) {
            assert!(clock.ack_queued(&window));
            offers.push(window);
        }
        assert_eq!(
            offers
                .iter()
                .map(|window| {
                    (
                        window.request_range.sample_start,
                        window.request_range.sample_end,
                    )
                })
                .collect::<Vec<_>>(),
            vec![(0, 4_000), (3_000, 4_500), (6_000, 10_000), (9_000, 11_000)]
        );
        assert_eq!(
            offers
                .iter()
                .map(|window| (window.admit_sample_start, window.admit_sample_end))
                .collect::<Vec<_>>(),
            vec![
                (0, 4_000),
                (4_000, 4_500),
                (6_000, 10_000),
                (10_000, 11_000)
            ]
        );
    }

    #[test]
    fn unmeasured_or_foreign_pcm_cannot_offer_an_l1_window() {
        let mut clock = OpenSpeechWindowClock::new(CaptureEvidenceIdentity::new("take", 7));
        let speech = measured_speech("take", 7, 5_000, &[(0, 5_000)]);
        let offered = clock.next(&speech, 1_000, None, false).unwrap();
        let foreign = measured_speech("other", 7, 5_000, &[(0, 5_000)]);
        assert!(clock.next(&foreign, 1_000, None, false).is_none());
        assert!(!clock.ack_queued(&offered));
        let unavailable = AcousticSpeechEvidence::unavailable(
            CaptureEvidenceIdentity::new("take", 7),
            SILERO_RAW_BOUNDARIES_PRODUCER,
            AcousticAvailability::Discontinuous {
                observed_samples: 5_000,
            },
        );
        assert!(clock.next(&unavailable, 1_000, None, false).is_none());
        assert_eq!(clock.next(&speech, 1_000, None, false), Some(offered));
    }

    #[test]
    fn later_unavailable_pcm_revokes_pending_l1_offer() {
        for availability in [
            AcousticAvailability::InvalidMeasurement {
                valid_samples: 4_000,
            },
            AcousticAvailability::Discontinuous {
                observed_samples: 4_000,
            },
        ] {
            let mut clock = OpenSpeechWindowClock::new(CaptureEvidenceIdentity::new("take", 7));
            let speech = measured_speech("take", 7, 4_000, &[(0, 4_000)]);
            let offered = clock.next(&speech, 1_000, None, false).unwrap();
            let unavailable = AcousticSpeechEvidence::unavailable(
                CaptureEvidenceIdentity::new("take", 7),
                SILERO_RAW_BOUNDARIES_PRODUCER,
                availability,
            );
            assert!(clock.next(&unavailable, 1_000, None, false).is_none());
            assert!(
                !clock.ack_queued(&offered),
                "whole-take measurement refusal must invalidate a prior pending offer: {availability:?}"
            );
        }
    }

    #[test]
    fn capture_energy_is_not_a_silero_boundary_observer() {
        let mut clock = OpenSpeechWindowClock::new(CaptureEvidenceIdentity::new("take", 7));
        let energy = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("take", 7),
            crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER,
            AcousticAvailability::Observed {
                observed_samples: 5_000,
            },
            vec![TailSampleRange {
                session: "take".into(),
                capture_epoch: 7,
                sample_start: 0,
                sample_end: 5_000,
            }],
        );
        assert!(clock.next(&energy, 1_000, None, false).is_none());
    }

    #[test]
    fn padded_coverage_evidence_is_not_an_admit_range() {
        let mut clock = OpenSpeechWindowClock::new(CaptureEvidenceIdentity::new("take", 7));
        let padded = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("take", 7),
            super::super::silero_fusion::SILERO_BOUNDARIES_PRODUCER,
            AcousticAvailability::Observed {
                observed_samples: 5_000,
            },
            vec![TailSampleRange {
                session: "take".into(),
                capture_epoch: 7,
                sample_start: 0,
                sample_end: 5_000,
            }],
        );
        assert!(clock.next(&padded, 1_000, None, false).is_none());
    }

    #[test]
    fn measured_silent_tail_does_not_become_decoder_audio() {
        let mut clock = OpenSpeechWindowClock::new(CaptureEvidenceIdentity::new("take", 7));
        let speech_then_silence = measured_speech("take", 7, 9_000, &[(0, 4_000)]);
        let window = clock.next(&speech_then_silence, 1_000, None, true).unwrap();
        assert_eq!(window.request_range.sample_end, 4_000);
        assert!(clock.ack_queued(&window));
        assert!(
            clock
                .next(&speech_then_silence, 1_000, None, true)
                .is_none()
        );
    }

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
