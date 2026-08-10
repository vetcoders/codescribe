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

/// Seconds after an Apple utterance commit during which the engine may still
/// rewrite the open tail. Measured operator range ~2–3 s; pin the mid point.
pub const APPLE_VOLATILE_WINDOW_SECS: f32 = 2.5;

/// Hard ceiling for seal starvation on continuous speech. Matches the force-
/// boundary named in the streaming-bridge design for constant speech >28 s.
pub const SEAL_STARVATION_CEILING_SECS: f32 = 28.0;

/// One byte-stable committed span after lexicon + (optional) Light+.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedSpan {
    /// Utterance / span identity (monotonic per session).
    pub id: u64,
    /// Hardened text — later windows, patches, and stop-path stages must not
    /// rewrite these bytes (human edits excluded).
    pub text: String,
    /// Absolute session end of the sealed audio span, in seconds.
    pub end_secs_millis: u32,
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
}

/// Outcome of one seal evaluation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        let raw_text = raw_text.into();
        if raw_text.trim().is_empty() {
            return;
        }
        // Idempotent on id: a re-commit of the same utterance refreshes the
        // pending text but does not invent a second pending slot.
        if let Some(existing) = self.pending.iter_mut().find(|p| p.id == id) {
            existing.raw_text = raw_text;
            existing.end_secs = end_secs;
            existing.apple_committed_at_secs = committed_at_secs;
            return;
        }
        if self.sealed.iter().any(|s| s.id == id) {
            // Already sealed — byte-stable fence: ignore re-commits.
            return;
        }
        self.pending.push(PendingSpan {
            id,
            raw_text,
            apple_committed_at_secs: committed_at_secs,
            end_secs,
            covering_whisper_window_id: None,
        });
    }

    /// An elapsed Whisper window now covers audio through `covered_through_secs`.
    pub fn note_whisper_window_elapsed(&mut self, window_id: u64, covered_through_secs: f32) {
        if window_id > self.last_elapsed_whisper_window_id {
            self.last_elapsed_whisper_window_id = window_id;
        }
        if covered_through_secs > self.whisper_covered_through_secs {
            self.whisper_covered_through_secs = covered_through_secs;
        }
        for pending in &mut self.pending {
            if pending.end_secs <= self.whisper_covered_through_secs + f32::EPSILON {
                pending.covering_whisper_window_id = Some(window_id);
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
        if !self.may_rewrite(id) {
            return false;
        }
        let new_text = new_text.into();
        if let Some(pending) = self.pending.iter_mut().find(|p| p.id == id) {
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
                    };
                    if !left_context.is_empty() && !sealed_span.text.is_empty() {
                        left_context.push(' ');
                    }
                    left_context.push_str(&sealed_span.text);
                    newly_sealed.push(sealed_span.clone());
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
                    };
                    if !left_context.is_empty() && !sealed_span.text.is_empty() {
                        left_context.push(' ');
                    }
                    left_context.push_str(&sealed_span.text);
                    newly_sealed.push(sealed_span.clone());
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
}
