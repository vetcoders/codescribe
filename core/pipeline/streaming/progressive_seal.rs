//! Progressive seal pipeline — harden spans during the live session.
//!
//! Operator layers 3–4 (2026-08-10): once both engines have closed a span, it
//! seals as `lexicon → Light+ → committed` and is never revisited. The stop
//! path becomes residual fill of the last unsealed span from session
//! partials — never a fresh full-file re-decode when the live lane stayed up.
//!
//! Double-seal condition (both required):
//! 1. Covered by a fully-elapsed Whisper window (`whisper_covered_through`
//!    past the span end), using the window IDs from streaming-bridge-v2.
//! 2. Outside Apple's volatile rewrite window (~2.5 s after the utterance
//!    commit) so the canvas text is no longer mid-rewrite.
//!
//! Recovery: if continuous speech keeps Apple's volatile window open past
//! [`SEAL_STARVATION_CEILING_SECS`], a time ceiling fires with telemetry
//! rather than a silent early seal.

use crate::pipeline::light_plus;
use crate::pipeline::stream_postprocess;
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailRequestIdentity,
    TailSampleRange, TailTimingQuality, TimedTailSegment,
};

use super::span_idempotence::{self, SpanIdempotenceLedger, SpanOffer};

/// Seconds after an Apple utterance commit during which the engine may still
/// rewrite the open tail. Measured operator range ~2–3 s; pin the mid point.
pub const APPLE_VOLATILE_WINDOW_SECS: f32 = 2.5;

/// Hard ceiling for seal starvation on continuous speech. Matches the force-
/// boundary named in the streaming-bridge design for constant speech >28 s.
pub const SEAL_STARVATION_CEILING_SECS: f32 = 28.0;

/// One byte-stable committed span after lexicon + (optional) Light+.
#[derive(Debug, Clone, PartialEq)]
pub struct SealedSpan {
    /// Utterance / span identity (monotonic per session).
    pub id: u64,
    /// Hardened text — later windows, patches, and stop-path stages must not
    /// rewrite these bytes (human edits excluded).
    pub text: String,
    /// Absolute session end of the sealed audio span, in seconds.
    pub end_secs_millis: u32,
    /// Canonical half-open PCM range for this sealed Apple span.
    pub range: TailSampleRange,
    /// Apple word/segment evidence pinned to the same PCM clock.
    pub words: Vec<TimedTailSegment>,
    /// Typed Apple evidence recorded before any later fusion policy.
    pub apple_evidence: TailProviderEvidence,
    /// Typed Whisper evidence for the covering window, when Layer 1 ran.
    pub whisper_evidence: Option<TailProviderEvidence>,
    /// Whisper segments mapped back to the capture PCM clock.
    pub whisper_words: Vec<TimedTailSegment>,
    /// Silero utterance this span was bound to, when the spectrum had an edge
    /// enclosing it. `Some` is the evidence that [`Self::range`] came from the
    /// VAD spectrum rather than from Apple's own segment boundaries; `None`
    /// records the fail-open case, never a dropped span.
    pub silero_utterance_id: Option<u64>,
}

impl SealedSpan {
    /// End time as seconds (f32) for comparisons against window clocks.
    pub fn end_secs(&self) -> f32 {
        self.end_secs_millis as f32 / 1000.0
    }
}

/// A span that has been heard but is not yet double-closed.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingSpan {
    pub id: u64,
    /// Raw engine text (or lexicon-ready canvas text) awaiting the seal pass.
    pub raw_text: String,
    /// Session time when Apple committed the utterance.
    pub apple_committed_at_secs: f32,
    /// Absolute end of the span in session audio seconds.
    pub end_secs: f32,
    /// Whisper window that fully covers this span, once known.
    pub covering_whisper_window_id: Option<u64>,
    pub range: TailSampleRange,
    pub words: Vec<TimedTailSegment>,
    pub apple_evidence: TailProviderEvidence,
    pub whisper_evidence: Option<TailProviderEvidence>,
    pub whisper_words: Vec<TimedTailSegment>,
    /// Silero utterance this span was bound to. Carried to [`SealedSpan`].
    pub silero_utterance_id: Option<u64>,
}

/// One Apple commit offered to the machine, with its PCM and identity
/// provenance. A record rather than a nine-argument call: every field is
/// provenance for the same span, and a positional list of that length is how
/// a range and an identity end up silently swapped.
#[derive(Debug, Clone, PartialEq)]
pub struct AppleCommit {
    /// Span identity (monotonic per session; from the Silero ledger when the
    /// fusion lane minted it, otherwise reserved from the same id space).
    pub id: u64,
    /// Raw engine text (or lexicon-ready canvas text) awaiting the seal pass.
    pub raw_text: String,
    /// Absolute end of the span in session audio seconds.
    pub end_secs: f32,
    /// Session time when Apple committed the utterance.
    pub committed_at_secs: f32,
    /// Canonical half-open PCM range for the span.
    pub range: TailSampleRange,
    /// Apple word/segment evidence pinned to the same PCM clock.
    pub words: Vec<TimedTailSegment>,
    /// Typed Apple evidence recorded before any later fusion policy.
    pub apple_evidence: TailProviderEvidence,
    /// Silero utterance the range was taken from, when one enclosed the span.
    pub silero_utterance_id: Option<u64>,
}

/// One live-lane partial retained for residual stop-path fill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPartial {
    pub text: String,
    /// Session end of the partial's coverage, in milliseconds.
    pub end_secs_millis: u32,
}

/// Progressive seal state machine over utterance / window IDs.
#[derive(Debug, Default, Clone)]
pub struct ProgressiveSealMachine {
    sealed: Vec<SealedSpan>,
    pending: Vec<PendingSpan>,
    /// Furthest session second fully covered by an elapsed Whisper window.
    whisper_covered_through_secs: f32,
    /// Highest Whisper window id observed as fully elapsed.
    last_elapsed_whisper_window_id: u64,
    /// Live partials retained for residual stop-path composition.
    session_partials: Vec<SessionPartial>,
    /// Times the starvation ceiling forced a seal evaluation.
    starvation_ceiling_hits: u64,
    /// Live lane health — when false, stop path may fall back to file inference.
    live_lane_alive: bool,
    /// Session-captured W13-4 flag. Restart-only configuration must not change
    /// underneath an active recording.
    span_idempotence_enabled: bool,
    /// W13-4 range-identity ledger. Consulted only when the lane flag is ON.
    span_idempotence: SpanIdempotenceLedger,
}

/// Outcome of one seal evaluation pass.
#[derive(Debug, Clone, PartialEq)]
pub struct SealTick {
    /// Spans that sealed on this tick (lexicon → Light+ applied).
    pub newly_sealed: Vec<SealedSpan>,
    /// True when the starvation ceiling participated in a seal decision.
    pub starvation_ceiling_used: bool,
}

/// Stop-path residual composition — seals + last unsealed partial tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopPathResidual {
    /// Delivered text = concatenation of seals + residual tail.
    pub text: String,
    /// True only when the residual had to fall back to a file-inference path
    /// because the live lane died mid-session.
    pub used_file_decode_fallback: bool,
    /// Sealed prefix bytes (immutable under residual append).
    pub sealed_prefix: String,
    /// Residual tail taken from session partials (empty when fully sealed).
    pub residual_tail: String,
}

impl ProgressiveSealMachine {
    /// Fresh machine for a new recording session.
    pub fn new() -> Self {
        Self {
            live_lane_alive: true,
            span_idempotence_enabled: span_idempotence::lane_enabled(),
            ..Self::default()
        }
    }

    /// Mark the live lane as dead — residual stop path may then use the
    /// PunctuationRepass / file-inference fallback.
    pub fn mark_live_lane_dead(&mut self) {
        self.live_lane_alive = false;
    }

    /// Whether the live lane is still considered healthy for residual fill.
    pub fn live_lane_alive(&self) -> bool {
        self.live_lane_alive
    }

    /// Record an Apple-committed utterance that is not yet progressive-sealed.
    pub fn note_apple_commit(
        &mut self,
        id: u64,
        raw_text: impl Into<String>,
        end_secs: f32,
        committed_at_secs: f32,
    ) {
        let end_sample = (end_secs.max(0.0) * 1_000.0).round() as u64;
        self.note_apple_commit_timed(AppleCommit {
            id,
            raw_text: raw_text.into(),
            end_secs,
            committed_at_secs,
            range: TailSampleRange {
                session: "legacy_progressive".to_string(),
                capture_epoch: 0,
                sample_start: 0,
                sample_end: end_sample,
            },
            words: Vec::new(),
            apple_evidence: TailProviderEvidence {
                source: TailEvidenceSource::AppleSpeech,
                revision: None,
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: None,
            },
            silero_utterance_id: None,
        });
    }

    /// Record an Apple commit together with canonical PCM, word and Silero
    /// identity provenance. This is data-only: seal eligibility and text
    /// transformation are the same as [`note_apple_commit`].
    pub fn note_apple_commit_timed(&mut self, commit: AppleCommit) -> bool {
        let AppleCommit {
            id,
            raw_text,
            end_secs,
            committed_at_secs,
            range,
            words,
            apple_evidence,
            silero_utterance_id,
        } = commit;
        if raw_text.trim().is_empty() {
            return false;
        }
        // Idempotent on id: a re-commit of the same utterance refreshes the
        // pending text but does not invent a second pending slot.
        if let Some(existing) = self.pending.iter_mut().find(|p| p.id == id) {
            existing.raw_text = raw_text;
            existing.end_secs = end_secs;
            existing.apple_committed_at_secs = committed_at_secs;
            existing.range = range;
            existing.words = words;
            existing.apple_evidence = apple_evidence;
            existing.silero_utterance_id = silero_utterance_id;
            return true;
        }
        if self.sealed.iter().any(|s| s.id == id) {
            // Already sealed — byte-stable fence: ignore re-commits.
            return false;
        }
        if self.span_idempotence_enabled {
            let verdict = self.span_idempotence.offer(SpanOffer {
                identity: TailRequestIdentity {
                    request_id: id,
                    range: range.clone(),
                },
                text: raw_text.clone(),
                timestamps_progressed: true,
                decode_ok: true,
            });
            if !verdict.lands_on_canvas() {
                return false;
            }
        }
        self.pending.push(PendingSpan {
            id,
            raw_text,
            apple_committed_at_secs: committed_at_secs,
            end_secs,
            covering_whisper_window_id: None,
            range,
            words,
            apple_evidence,
            whisper_evidence: None,
            whisper_words: Vec::new(),
            silero_utterance_id,
        });
        true
    }

    /// W13-4 receipts collected while the lane flag is ON.
    pub fn span_idempotence_receipts(&self) -> &[span_idempotence::SpanIdempotenceReceipt] {
        self.span_idempotence.receipts()
    }

    /// An elapsed Whisper window now covers audio through `covered_through_secs`.
    pub fn note_whisper_window_elapsed(&mut self, window_id: u64, covered_through_secs: f32) {
        self.note_whisper_window_elapsed_with_provenance(
            window_id,
            covered_through_secs,
            None,
            Vec::new(),
        );
    }

    /// Record elapsed coverage plus provider provenance without changing the
    /// existing double-close decision.
    pub fn note_whisper_window_elapsed_with_provenance(
        &mut self,
        window_id: u64,
        covered_through_secs: f32,
        evidence: Option<TailProviderEvidence>,
        words: Vec<TimedTailSegment>,
    ) {
        if window_id > self.last_elapsed_whisper_window_id {
            self.last_elapsed_whisper_window_id = window_id;
        }
        if covered_through_secs > self.whisper_covered_through_secs {
            self.whisper_covered_through_secs = covered_through_secs;
        }
        for pending in &mut self.pending {
            if pending.end_secs <= self.whisper_covered_through_secs + f32::EPSILON {
                pending.covering_whisper_window_id = Some(window_id);
                if pending.id == window_id {
                    pending.whisper_evidence = evidence.clone();
                    pending.whisper_words = words.clone();
                }
            }
        }
    }

    /// Retain a live partial for residual stop-path fill.
    pub fn note_session_partial(&mut self, text: impl Into<String>, end_secs: f32) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        let end_secs_millis = (end_secs.max(0.0) * 1000.0).round() as u32;
        self.session_partials.push(SessionPartial {
            text,
            end_secs_millis,
        });
    }

    /// Sealed spans in order.
    pub fn sealed_spans(&self) -> &[SealedSpan] {
        &self.sealed
    }

    /// Pending (unsealed) spans in order.
    pub fn pending_spans(&self) -> &[PendingSpan] {
        &self.pending
    }

    /// Concatenation of sealed text (streaming floor of progressive seals).
    pub fn sealed_prefix(&self) -> String {
        self.sealed
            .iter()
            .map(|s| s.text.as_str())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whether a later patch/window/stop-path stage may rewrite `id`.
    /// Sealed spans return false — the byte-stable fence.
    pub fn may_rewrite(&self, id: u64) -> bool {
        !self.sealed.iter().any(|s| s.id == id)
    }

    /// Attempt to rewrite a span. Sealed spans refuse (Ok(false)); pending
    /// accept (Ok(true)); unknown ids return Ok(false).
    pub fn try_rewrite(&mut self, id: u64, new_text: impl Into<String>) -> bool {
        let Some(range) = self
            .pending
            .iter()
            .find(|pending| pending.id == id)
            .map(|pending| pending.range.clone())
        else {
            return false;
        };
        self.try_rewrite_anchored(id, &range, new_text)
    }

    /// Rewrite only the pending span whose PCM identity acoustically overlaps
    /// the admitted evidence. Other anchored spans and their bytes are untouched.
    pub fn try_rewrite_anchored(
        &mut self,
        id: u64,
        evidence_range: &TailSampleRange,
        new_text: impl Into<String>,
    ) -> bool {
        if !self.may_rewrite(id) {
            return false;
        }
        let new_text = new_text.into();
        if let Some(pending) = self.pending.iter_mut().find(|p| p.id == id) {
            let same_clock = pending.range.session == evidence_range.session
                && pending.range.capture_epoch == evidence_range.capture_epoch;
            let overlaps = pending.range.sample_start < evidence_range.sample_end
                && evidence_range.sample_start < pending.range.sample_end;
            if !same_clock || !overlaps {
                return false;
            }
            pending.raw_text = new_text;
            return true;
        }
        false
    }

    /// Evaluate double-seal conditions and seal every ready span.
    ///
    /// `now_secs` is the session clock. `force_raw` skips Light+ (Ctrl-hold
    /// contract) but still runs lexicon and commits the span.
    pub fn try_seal(&mut self, now_secs: f32, force_raw: bool) -> SealTick {
        let mut newly_sealed = Vec::new();
        let mut starvation_ceiling_used = false;
        let mut left_context = self.sealed_prefix();

        // Drain ready pending spans in order so left context accumulates.
        let mut still_pending = Vec::with_capacity(self.pending.len());
        let pending = std::mem::take(&mut self.pending);
        for span in pending {
            let reason = self.seal_block_reason(&span, now_secs);
            match reason {
                None => {
                    let sealed = seal_span_text(&span.raw_text, &left_context, force_raw);
                    let sealed_span = SealedSpan {
                        id: span.id,
                        text: sealed,
                        end_secs_millis: (span.end_secs.max(0.0) * 1000.0).round() as u32,
                        range: span.range,
                        words: span.words,
                        apple_evidence: span.apple_evidence,
                        whisper_evidence: span.whisper_evidence,
                        whisper_words: span.whisper_words,
                        silero_utterance_id: span.silero_utterance_id,
                    };
                    if !left_context.is_empty() && !sealed_span.text.is_empty() {
                        left_context.push(' ');
                    }
                    left_context.push_str(&sealed_span.text);
                    newly_sealed.push(sealed_span.clone());
                    self.span_idempotence.mark_sealed(&sealed_span.range);
                    self.sealed.push(sealed_span);
                }
                Some(SealBlockReason::StarvationCeiling) => {
                    // Ceiling forces evaluation: seal with telemetry, do not
                    // silently seal early without the flag.
                    starvation_ceiling_used = true;
                    self.starvation_ceiling_hits = self.starvation_ceiling_hits.saturating_add(1);
                    let sealed = seal_span_text(&span.raw_text, &left_context, force_raw);
                    let sealed_span = SealedSpan {
                        id: span.id,
                        text: sealed,
                        end_secs_millis: (span.end_secs.max(0.0) * 1000.0).round() as u32,
                        range: span.range,
                        words: span.words,
                        apple_evidence: span.apple_evidence,
                        whisper_evidence: span.whisper_evidence,
                        whisper_words: span.whisper_words,
                        silero_utterance_id: span.silero_utterance_id,
                    };
                    if !left_context.is_empty() && !sealed_span.text.is_empty() {
                        left_context.push(' ');
                    }
                    left_context.push_str(&sealed_span.text);
                    newly_sealed.push(sealed_span.clone());
                    self.span_idempotence.mark_sealed(&sealed_span.range);
                    self.sealed.push(sealed_span);
                    tracing::info!(
                        span_id = span.id,
                        age_secs = now_secs - span.apple_committed_at_secs,
                        ceiling_secs = SEAL_STARVATION_CEILING_SECS,
                        "progressive_seal starvation ceiling — forced seal with telemetry"
                    );
                }
                Some(_) => still_pending.push(span),
            }
        }
        self.pending = still_pending;
        SealTick {
            newly_sealed,
            starvation_ceiling_used,
        }
    }

    /// Starvation-ceiling hit counter for telemetry / reports.
    pub fn starvation_ceiling_hits(&self) -> u64 {
        self.starvation_ceiling_hits
    }

    /// Residual tail text from session partials past the last sealed end.
    ///
    /// When nothing is sealed yet, the residual is the last partial (or the
    /// concatenation of partials). When seals exist, only partial coverage
    /// beyond the last seal end is returned.
    pub fn residual_tail_from_partials(&self) -> String {
        let sealed_end_millis = self.sealed.last().map(|s| s.end_secs_millis).unwrap_or(0);
        let mut parts: Vec<&str> = Vec::new();
        for partial in &self.session_partials {
            if partial.end_secs_millis > sealed_end_millis {
                let t = partial.text.trim();
                if !t.is_empty() {
                    parts.push(t);
                }
            }
        }
        // Prefer the freshest partial beyond the seal boundary — partials are
        // progressive rewrites of the same open tail, not independent appends.
        parts.last().copied().unwrap_or("").to_string()
    }

    /// Compose the stop-path delivery from progressive seals + residual
    /// session partials. Never performs file inference. Wall time is pure
    /// string work — target << 1 s on any realistic session.
    pub fn compose_stop_path_residual(&self) -> StopPathResidual {
        let sealed_prefix = self.sealed_prefix();
        let residual_tail = if self.pending.is_empty() {
            self.residual_tail_from_partials()
        } else {
            // Unsealed pending spans are the residual: use their raw text
            // joined, preferring the live partial when it is fresher.
            let pending_text = self
                .pending
                .iter()
                .map(|p| p.raw_text.trim())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let from_partials = self.residual_tail_from_partials();
            if from_partials.chars().count() > pending_text.chars().count() {
                from_partials
            } else {
                pending_text
            }
        };
        let text = crate::stt::append_tail_gap(&sealed_prefix, &residual_tail);
        StopPathResidual {
            text,
            used_file_decode_fallback: false,
            sealed_prefix,
            residual_tail,
        }
    }

    /// Seal every span still pending when the session itself ends.
    ///
    /// Both double-close gates are *vacuously* satisfied once capture is over:
    /// no later Apple callback can revise a span, and no further Whisper window
    /// can arrive. Holding a span past that point is not caution, it is a hang.
    ///
    /// # Why this exists as its own entry point
    ///
    /// The end-of-session caller used to reuse `try_seal(audio_secs + volatile
    /// window + epsilon)`. That clock is derived from the PCM sample counter,
    /// while `apple_committed_at_secs` comes from SFSpeech's own segment clock,
    /// which can sit a few milliseconds *ahead* of it. Measured 2026-08-12:
    /// audio clock 217.376s, last span committed at 217.378s → age 2.499s
    /// against a 2.5s volatile window. The span missed by one millisecond, and
    /// because the audio clock is frozen after EOF it could never age past the
    /// gate — not even into the starvation ceiling. The worker then burned the
    /// full 30s closure timeout waiting on a Whisper completion that would not
    /// have unblocked it anyway (`rec_stop=36.701s` in the stop-path budget).
    ///
    /// Anchoring on the spans' own timestamps instead of the audio clock keeps
    /// the volatile semantics exactly as written and removes the race.
    pub fn seal_remaining_at_session_end(&mut self, force_raw: bool) -> SealTick {
        let horizon = self
            .pending
            .iter()
            .map(|span| span.apple_committed_at_secs.max(span.end_secs))
            .fold(0.0_f32, f32::max);
        // No further window is coming, so everything recorded is as covered as
        // it will ever be — satisfy `whisper_ready` without inventing an id.
        self.whisper_covered_through_secs = self.whisper_covered_through_secs.max(horizon);
        let tick = self.try_seal(horizon + APPLE_VOLATILE_WINDOW_SECS + 0.001, force_raw);
        // The partial pool is spent once the end-of-session seal has run: every
        // word it held is either inside a sealed span or inside the open
        // partial the worker seals *before* calling this. Spans carry SFSpeech
        // segment-clock ends while partials carry the receipt clock, which
        // always runs slightly later — so a surviving partial reads as "past
        // the last seal" to `compose_stop_path_residual` and re-appends text
        // the seal already delivered. Live 2026-08-12 21:15: "Jaki chcesz.
        // Kos." arrived twice in an 8s take exactly this way.
        self.session_partials.clear();
        tick
    }

    /// Why a pending span is not yet sealable, or None when both engines closed it.
    fn seal_block_reason(&self, span: &PendingSpan, now_secs: f32) -> Option<SealBlockReason> {
        let age = now_secs - span.apple_committed_at_secs;
        let whisper_ready = span.covering_whisper_window_id.is_some()
            || span.end_secs <= self.whisper_covered_through_secs + f32::EPSILON;
        let apple_settled = age >= APPLE_VOLATILE_WINDOW_SECS;

        if whisper_ready && apple_settled {
            return None;
        }
        if age >= SEAL_STARVATION_CEILING_SECS {
            return Some(SealBlockReason::StarvationCeiling);
        }
        if !whisper_ready {
            return Some(SealBlockReason::WhisperWindowUnelapsed);
        }
        Some(SealBlockReason::AppleVolatile)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SealBlockReason {
    AppleVolatile,
    WhisperWindowUnelapsed,
    StarvationCeiling,
}

/// Lexicon → Light+ (unless `force_raw`) on one span, with left context so
/// casing at a sentence start sees the preceding terminal.
pub fn seal_span_text(raw: &str, left_context: &str, force_raw: bool) -> String {
    let after_lexicon = stream_postprocess::apply_lexicon(raw);
    let trimmed = after_lexicon.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if force_raw {
        return trimmed.to_string();
    }
    light_plus::apply_with_left_context(left_context, trimmed)
}

/// Whether Smart-mode stop path may consume session partials instead of
/// re-transcribing the file. Live lane must still be up.
pub fn residual_prefers_session_partials(
    live_lane_alive: bool,
    has_residual_partials: bool,
) -> bool {
    live_lane_alive && has_residual_partials
}

#[cfg(test)]
mod progressive_seal_tests {
    use super::*;

    fn machine_with_double_closed_span(raw: &str) -> ProgressiveSealMachine {
        let mut m = ProgressiveSealMachine::new();
        m.note_apple_commit(1, raw, 5.0, 5.0);
        m.note_whisper_window_elapsed(1, 8.0);
        // Past Apple volatile window.
        let _ = m.try_seal(5.0 + APPLE_VOLATILE_WINDOW_SECS + 0.1, false);
        m
    }

    /// A sealed span is byte-stable: later patches and re-commits cannot rewrite it.
    #[test]
    fn progressive_seal_byte_stable_after_commit() {
        let mut m = machine_with_double_closed_span("pierwsze zdanie");
        assert_eq!(m.sealed_spans().len(), 1);
        let sealed_text = m.sealed_spans()[0].text.clone();
        assert!(!m.may_rewrite(1), "sealed span must refuse rewrite");
        assert!(
            !m.try_rewrite(1, "HACKED TEXT THAT MUST NOT LAND"),
            "try_rewrite on a sealed id must fail closed"
        );
        assert_eq!(
            m.sealed_spans()[0].text,
            sealed_text,
            "sealed bytes must be unchanged after a refused rewrite"
        );
        // Re-commit of the same id is ignored.
        m.note_apple_commit(1, "completely different", 6.0, 10.0);
        assert_eq!(m.sealed_spans()[0].text, sealed_text);
        assert!(m.pending_spans().is_empty());
    }

    #[test]
    fn anchored_rewrite_requires_overlap_and_preserves_adjacent_span_bytes() {
        let mut machine = ProgressiveSealMachine::new();
        assert!(machine.note_apple_commit_timed(AppleCommit {
            id: 1,
            raw_text: "pierwszy".into(),
            end_secs: 1.0,
            committed_at_secs: 1.0,
            range: TailSampleRange {
                session: "overlap".into(),
                capture_epoch: 7,
                sample_start: 0,
                sample_end: 16_000,
            },
            words: Vec::new(),
            apple_evidence: TailProviderEvidence {
                source: crate::stt::tail_provider::TailEvidenceSource::AppleSpeech,
                revision: None,
                stability: crate::stt::tail_provider::TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: None,
            },
            silero_utterance_id: None,
        }));
        assert!(machine.note_apple_commit_timed(AppleCommit {
            id: 2,
            raw_text: "drugi".into(),
            end_secs: 2.0,
            committed_at_secs: 2.0,
            range: TailSampleRange {
                session: "overlap".into(),
                capture_epoch: 7,
                sample_start: 16_000,
                sample_end: 32_000,
            },
            words: Vec::new(),
            apple_evidence: TailProviderEvidence {
                source: crate::stt::tail_provider::TailEvidenceSource::AppleSpeech,
                revision: None,
                stability: crate::stt::tail_provider::TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::Synthetic,
                avg_logprob: None,
            },
            silero_utterance_id: None,
        }));
        let disjoint = TailSampleRange {
            session: "overlap".into(),
            capture_epoch: 7,
            sample_start: 16_000,
            sample_end: 32_000,
        };
        assert!(!machine.try_rewrite_anchored(1, &disjoint, "floating"));
        let overlapping = TailSampleRange {
            session: "overlap".into(),
            capture_epoch: 7,
            sample_start: 8_000,
            sample_end: 16_000,
        };
        assert!(machine.try_rewrite_anchored(1, &overlapping, "poprawiony"));
        assert_eq!(machine.pending_spans()[0].raw_text, "poprawiony");
        assert_eq!(machine.pending_spans()[1].raw_text, "drugi");
    }

    /// Seal ordering is lexicon → Light+: a lexicon-corrected word at a
    /// sentence start receives correct casing from the left-context Light+ pass.
    #[test]
    fn progressive_seal_ordering_lexicon_before_light_plus() {
        // Builtin lexicon rewrites "doker" → "Docker". The span opens a new
        // sentence after a terminal in left context, so Light+ must capitalise
        // the lexicon result ("Docker…"), not the raw mishear.
        let left = "Koniec poprzedniego.";
        let raw = "doker i kubernets w produkcji";
        let sealed = seal_span_text(raw, left, false);
        // Lexicon first: doker→Docker, kubernets→Kubernetes (builtin set).
        assert!(
            sealed.contains("Docker") || sealed.to_lowercase().contains("docker"),
            "lexicon must rewrite before Light+: {sealed}"
        );
        // Light+ with left context that ends in a terminal: the span starts a
        // new sentence so the first letter rises.
        let first_alpha = sealed.chars().find(|c| c.is_alphabetic());
        assert!(
            first_alpha.is_some_and(|c| c.is_uppercase()),
            "Light+ must capitalise sentence start after lexicon: {sealed}"
        );
        // force_raw skips Light+ casing but still seals words (lexicon may still run).
        let raw_sealed = seal_span_text(raw, left, true);
        let raw_first = raw_sealed.chars().find(|c| c.is_alphabetic());
        // Raw path does not force a trailing period or sentence capitalisation
        // the way Light+ does — if the lexicon output is lowercase-start, it stays.
        assert!(
            !raw_sealed.ends_with('.')
                || raw_first.is_some_and(|c| c.is_lowercase())
                || raw_sealed != sealed,
            "force_raw path must differ from Light+ shaping: raw={raw_sealed} shaped={sealed}"
        );
    }

    /// Double-seal: volatile Apple tail OR un-elapsed Whisper window blocks seal.
    #[test]
    fn progressive_seal_double_condition_blocks_volatile_or_unelapsed() {
        let mut m = ProgressiveSealMachine::new();
        m.note_apple_commit(1, "jeszcze lotne", 3.0, 3.0);

        // No Whisper coverage yet — must not seal even past volatile window.
        let after_volatile = 3.0 + APPLE_VOLATILE_WINDOW_SECS + 1.0; // 6.5
        let tick = m.try_seal(after_volatile, false);
        assert!(
            tick.newly_sealed.is_empty(),
            "un-elapsed Whisper window must block seal"
        );
        assert_eq!(m.pending_spans().len(), 1);

        // Whisper covers span 1. A fresh span 2 is still inside its volatile window.
        m.note_whisper_window_elapsed(1, 10.0);
        m.note_apple_commit(2, "drugi lotny", 7.0, 7.0);
        let tick = m.try_seal(7.2, false); // age(span2)=0.2 < volatile
        // Span 1: whisper ready + apple settled (age 4.2) → seals.
        // Span 2: volatile → stays pending.
        assert!(
            m.sealed_spans().iter().any(|s| s.id == 1),
            "span 1 must seal once both engines closed it: {:?}",
            tick.newly_sealed
        );
        assert!(
            m.pending_spans().iter().any(|p| p.id == 2),
            "span 2 still inside Apple volatile window must NOT seal"
        );

        // Advance past volatile for #2 — now seals.
        let tick = m.try_seal(7.0 + APPLE_VOLATILE_WINDOW_SECS + 0.1, false);
        assert!(
            tick.newly_sealed.iter().any(|s| s.id == 2),
            "span 2 seals after volatile window closes"
        );
        assert!(m.pending_spans().is_empty());
    }

    /// The 2026-08-12 stop-path hang, reduced to its arithmetic.
    ///
    /// SFSpeech committed the last span at 217.378s on its own segment clock
    /// while the PCM counter had reached 217.376s. The end-of-session seal used
    /// the audio clock, so the span's age came out at 2.499s against a 2.5s
    /// volatile window — short by one millisecond, and frozen there forever
    /// because the audio clock stops advancing at EOF. The worker then sat on
    /// its closure timeout, costing the operator 30s on a stop that owed
    /// nothing (`rec_stop=36.701s`).
    #[test]
    fn session_end_seals_the_span_a_frozen_audio_clock_holds_forever() {
        let mut m = ProgressiveSealMachine::new();
        let audio_eof_secs = 217.376_f32;
        let apple_commit_secs = 217.378_f32;
        m.note_apple_commit(48, "ostatnie słowo", apple_commit_secs, apple_commit_secs);
        // Whisper closed its side — the volatile gate is the only thing left.
        m.note_whisper_window_elapsed(48, apple_commit_secs);

        let frozen = m.try_seal(audio_eof_secs + APPLE_VOLATILE_WINDOW_SECS + 0.001, false);
        assert!(
            frozen.newly_sealed.is_empty(),
            "regression guard: this clock is exactly the one that hung, it must still miss"
        );
        assert_eq!(
            m.pending_spans().len(),
            1,
            "the span the old end-of-session clock could never release"
        );

        let at_end = m.seal_remaining_at_session_end(false);
        assert_eq!(
            at_end.newly_sealed.len(),
            1,
            "session end must seal on the span's own clock, not the audio counter"
        );
        assert!(
            m.pending_spans().is_empty(),
            "no span may outlive the session that produced it"
        );
    }

    /// The 2026-08-12 21:15 live duplicate ("Jaki chcesz. Kos." delivered
    /// twice): spans sealed at session end carry SFSpeech segment-clock ends,
    /// while session partials carry the receipt clock, which always runs a
    /// little later. The stop-path residual then saw the freshest partial as
    /// "past the last seal" and appended text the seal already delivered.
    /// After an end-of-session seal the partial pool must be empty — every
    /// word it held is either in a span or in the open partial the worker
    /// seals first.
    #[test]
    fn session_end_seal_leaves_no_partial_for_the_residual_to_duplicate() {
        let mut m = ProgressiveSealMachine::new();
        m.note_apple_commit(2, "jaki chcesz kos", 8.202, 8.202);
        m.note_whisper_window_elapsed(2, 8.202);
        // Receipt-clock partial restating the same tail, "later" than the seal.
        m.note_session_partial("jaki chcesz kos", 8.4);

        let tick = m.seal_remaining_at_session_end(false);
        assert_eq!(tick.newly_sealed.len(), 1);

        let residual = m.compose_stop_path_residual();
        assert_eq!(
            residual.residual_tail, "",
            "a partial restating sealed text must not ride the residual back in"
        );
        assert_eq!(
            residual.text.matches("chcesz").count(),
            1,
            "the delivered text must carry the phrase exactly once: {:?}",
            residual.text
        );
    }

    /// Session end also closes the Whisper gate: once capture stops, no further
    /// window can arrive, so holding a span for one is waiting on nothing.
    #[test]
    fn session_end_seals_span_that_never_got_a_whisper_window() {
        let mut m = ProgressiveSealMachine::new();
        m.note_apple_commit(1, "bez lat ki", 10.0, 10.0);

        let mid_session = m.try_seal(10.0 + APPLE_VOLATILE_WINDOW_SECS + 0.1, false);
        assert!(
            mid_session.newly_sealed.is_empty(),
            "mid-session the un-elapsed Whisper window must still block"
        );

        let at_end = m.seal_remaining_at_session_end(false);
        assert_eq!(
            at_end.newly_sealed.len(),
            1,
            "at session end there is no window left to wait for"
        );
    }

    /// Ctrl-hold force_raw skips Light+ but still seals words.
    #[test]
    fn progressive_seal_force_raw_skips_light_plus_still_seals() {
        let mut m = ProgressiveSealMachine::new();
        m.note_apple_commit(1, "surowy tekst bez kropki", 2.0, 2.0);
        m.note_whisper_window_elapsed(1, 5.0);
        let tick = m.try_seal(2.0 + APPLE_VOLATILE_WINDOW_SECS + 0.1, true);
        assert_eq!(tick.newly_sealed.len(), 1);
        let text = &tick.newly_sealed[0].text;
        // Light+ would append a terminal period and capitalise — force_raw must not.
        assert!(
            !text.ends_with('.'),
            "force_raw must skip Light+ terminal: {text}"
        );
        assert!(
            text.starts_with('s') || text.chars().next().is_some_and(|c| c.is_lowercase()),
            "force_raw must skip Light+ capitalisation: {text}"
        );
        assert!(!m.may_rewrite(1), "force_raw still seals (byte-stable)");
    }

    /// Residual stop path concatenates seals + residual partials without file decode.
    #[test]
    fn stop_path_residual_from_session_partials_no_file_decode() {
        let mut m = ProgressiveSealMachine::new();
        m.note_apple_commit(1, "pierwsze zdanie", 3.0, 3.0);
        m.note_whisper_window_elapsed(1, 6.0);
        let _ = m.try_seal(3.0 + APPLE_VOLATILE_WINDOW_SECS + 0.1, false);

        // Open residual covered only by live partials (no second seal yet).
        m.note_session_partial("ogon z partiali", 8.0);
        let residual = m.compose_stop_path_residual();
        assert!(
            !residual.used_file_decode_fallback,
            "healthy live lane must not fall back to file decode"
        );
        assert!(
            residual.text.contains("ogon z partiali") || residual.residual_tail.contains("ogon"),
            "residual must surface session partials: {:?}",
            residual
        );
        // Sealed prefix is preserved as the leading bytes.
        assert!(
            residual.text.starts_with(&residual.sealed_prefix) || residual.sealed_prefix.is_empty(),
            "delivered text must keep sealed prefix: {:?}",
            residual
        );
        assert!(
            residual_prefers_session_partials(true, true),
            "alive lane + partials → residual from partials"
        );
        assert!(
            !residual_prefers_session_partials(false, true),
            "dead live lane must not claim partial residual"
        );
    }

    /// Fixture-style residual composition is pure string work (<< 1 s).
    #[test]
    fn stop_path_residual_phase_under_one_second_on_fixture_replay() {
        let mut m = ProgressiveSealMachine::new();
        // Simulate a multi-seal session with a residual tail — the shape of the
        // 2026-08-10 stop path that used to spend 8.458 s on fresh file inference.
        for i in 1..=12 {
            let t = i as f32 * 4.0;
            m.note_apple_commit(i, format!("zdanie numer {i} o Codescribe"), t, t);
            m.note_whisper_window_elapsed(i, t + 4.0);
            let _ = m.try_seal(t + APPLE_VOLATILE_WINDOW_SECS + 0.2, false);
        }
        m.note_session_partial("ostatni ogon z live partiali bez re-decode", 55.0);

        let started = std::time::Instant::now();
        let residual = m.compose_stop_path_residual();
        let phase_secs = started.elapsed().as_secs_f64();

        assert!(
            phase_secs < 1.0,
            "residual final_pass phase must be < 1 s (measured {phase_secs:.6}s; baseline was 8.458s)"
        );
        assert!(!residual.used_file_decode_fallback);
        assert_eq!(
            residual.text,
            crate::stt::append_tail_gap(&residual.sealed_prefix, &residual.residual_tail)
        );
        assert!(
            m.sealed_spans().len() >= 10,
            "fixture must have sealed the bulk of the session"
        );
        // Emit the measured number so the report can quote it.
        eprintln!(
            "stop_path_residual_phase_secs={phase_secs:.6} baseline=8.458 sealed={}",
            m.sealed_spans().len()
        );
    }

    #[test]
    fn w13_live_seal_refuses_replayed_range_identity_when_armed() {
        let range = TailSampleRange {
            session: "w13-4-live".into(),
            capture_epoch: 1,
            sample_start: 0,
            sample_end: 8_000,
        };
        let evidence = TailProviderEvidence {
            source: TailEvidenceSource::AppleSpeech,
            revision: None,
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::Synthetic,
            avg_logprob: None,
        };
        let mut m = ProgressiveSealMachine::new();
        m.span_idempotence_enabled = true;
        assert!(m.note_apple_commit_timed(AppleCommit {
            id: 1,
            raw_text: "fragment odzyskany".into(),
            end_secs: 0.5,
            committed_at_secs: 0.5,
            range: range.clone(),
            words: Vec::new(),
            apple_evidence: evidence.clone(),
            silero_utterance_id: Some(1),
        }));
        m.note_whisper_window_elapsed(1, 4.0);
        let tick = m.try_seal(APPLE_VOLATILE_WINDOW_SECS + 1.0, true);
        assert_eq!(tick.newly_sealed.len(), 1);
        assert_eq!(
            tick.newly_sealed[0].silero_utterance_id,
            Some(1),
            "the Silero identity a span was bound to must survive the seal"
        );

        let replayed = m.note_apple_commit_timed(AppleCommit {
            id: 2,
            raw_text: "fragment odzyskany".into(),
            end_secs: 0.5,
            committed_at_secs: 0.5,
            range,
            words: Vec::new(),
            apple_evidence: evidence,
            silero_utterance_id: Some(2),
        });
        assert!(!replayed, "new Apple id on a sealed range must be refused");
        assert_eq!(m.pending_spans().len(), 0);
        assert_eq!(m.sealed_spans().len(), 1);
        assert_eq!(m.sealed_prefix(), "fragment odzyskany");
        assert!(
            m.span_idempotence_receipts()
                .iter()
                .any(|r| r.code == "replayed_range_identity")
        );
    }
}
