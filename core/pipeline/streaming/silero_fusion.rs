//! W13-3B — Silero utterance identity + conservative per-word fusion.
//!
//! Lane flag [`SILERO_FUSION_ENV`] is **default OFF**. When armed:
//! Silero Supervisor edges mint utterance identity on the PCM sample clock;
//! Apple cumulative finals are sliced onto those ranges by time; Whisper and
//! Apple then fuse conservatively (agreements + clear gap fills). Unresolved
//! alternatives are receipted, never confidence-arbitrated. Every write into
//! a pending span goes through [`super::progressive_seal::ProgressiveSealMachine::try_rewrite`].
//!
//! # One Silero per session
//!
//! [`SileroIngress`] is the session's **only** `SpeechSession`. Both consumers
//! of speech edges read it: the fusion ledger (utterance identity) and the
//! Apple engine lifecycle (`EpochGate` wake/sleep). Two independent VAD
//! sessions over the same PCM would mean two spectra and two sets of
//! boundaries, and "the same utterance" would then mean two different sample
//! ranges depending on which consumer was asked. [`SileroIngress::observe`] is
//! the single decision point that derives both from one observation.

use crate::audio::chunker::{SpeechEvent, SpeechSession};
use crate::stt::tail_patcher::SkipReasonCode;
use crate::stt::tail_provider::{TailSampleRange, TimedTailSegment};

/// Lane flag for Silero-identity conservative fusion. Unset / `0` / `false` /
/// `off` / `no` keep the existing production path bit-identical.
pub const SILERO_FUSION_ENV: &str = "CODESCRIBE_SILERO_FUSION";

/// Bounded-context A/B selector. Never crosses a long-silence cut.
pub const SILERO_FUSION_CONTEXT_ENV: &str = "CODESCRIBE_SILERO_FUSION_CONTEXT";

/// Silence longer than this (samples at the capture rate) is a hard context
/// fence — left-audio pad must not reach across it.
pub const LONG_SILENCE_FENCE_SECS: f32 = 0.55;

/// Default left-audio pad when [`FusionContextMode::LeftAudioPad`] is armed.
pub const DEFAULT_LEFT_PAD_SECS: f32 = 0.40;

/// Whether the W13-3B fusion lane is armed. Default OFF pending the operator's
/// live A/B decision required by the original engine roadmap.
pub fn lane_enabled() -> bool {
    let raw = std::env::var(SILERO_FUSION_ENV).ok();
    lane_enabled_from_raw(raw.as_deref())
}

fn lane_enabled_from_raw(raw: Option<&str>) -> bool {
    raw.is_some_and(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// One Silero-bounded utterance on the session PCM clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SileroUtterance {
    pub id: u64,
    pub range: TailSampleRange,
    pub closed: bool,
}

/// Ledger of Silero-minted utterance identities. Pure data; the Supervisor
/// machine in [`SileroIngress`] is the only writer in production.
#[derive(Debug, Clone, Default)]
pub struct UtteranceLedger {
    next_id: u64,
    utterances: Vec<SileroUtterance>,
}

impl UtteranceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint (or refresh) an open utterance covering `[sample_start, sample_end)`.
    pub fn open_or_extend(
        &mut self,
        session: &str,
        capture_epoch: u64,
        sample_start: u64,
        sample_end: u64,
    ) -> u64 {
        let sample_end = sample_end.max(sample_start);
        if let Some(open) = self.utterances.iter_mut().rev().find(|u| !u.closed) {
            open.range.sample_end = sample_end.max(open.range.sample_end);
            return open.id;
        }
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.utterances.push(SileroUtterance {
            id,
            range: TailSampleRange {
                session: session.to_string(),
                capture_epoch,
                sample_start,
                sample_end,
            },
            closed: false,
        });
        id
    }

    /// Close the open utterance so the next speech edge mints a new identity.
    pub fn close_open(&mut self, sample_end: u64) -> Option<u64> {
        let open = self.utterances.iter_mut().rev().find(|u| !u.closed)?;
        open.range.sample_end = sample_end.max(open.range.sample_end);
        open.closed = true;
        Some(open.id)
    }

    pub fn utterances(&self) -> &[SileroUtterance] {
        &self.utterances
    }

    /// Utterance whose range contains `sample` (half-open). Prefers the
    /// tightest closed span; falls back to the open span.
    pub fn utterance_covering(&self, sample: u64) -> Option<&SileroUtterance> {
        self.utterances
            .iter()
            .filter(|u| u.range.sample_start <= sample && sample < u.range.sample_end)
            .min_by_key(|u| u.range.sample_end.saturating_sub(u.range.sample_start))
    }

    /// Tightest utterance that fully **encloses** `[sample_start, sample_end)`.
    ///
    /// This is the seal-time binding query: an Apple span may adopt a Silero
    /// range only when the spectrum edge already covers every sample Apple
    /// claimed. Mere overlap is refused on purpose — adopting a range that
    /// starts after Apple's first word would hand Layer 1 a window over audio
    /// the utterance never contained, and the span would seal against a decode
    /// of the wrong seconds. No enclosure ⇒ the caller keeps its own range
    /// (fail-open; content is never dropped for want of an edge).
    pub fn utterance_enclosing(
        &self,
        sample_start: u64,
        sample_end: u64,
    ) -> Option<&SileroUtterance> {
        let sample_end = sample_end.max(sample_start);
        self.utterances
            .iter()
            .filter(|u| u.range.sample_start <= sample_start && sample_end <= u.range.sample_end)
            .min_by_key(|u| u.range.sample_end.saturating_sub(u.range.sample_start))
    }

    /// Burn one identity without minting an utterance.
    ///
    /// The Apple-boundary fallback still needs a span id, and it must not be an
    /// id Silero will later mint for a real utterance: `note_apple_commit_timed`
    /// is idempotent on id, so a collision would silently merge an Apple span
    /// with an unrelated Silero one. One ledger, one id space.
    pub fn reserve_id(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }
}

/// What one observed capture chunk means to every consumer of the session's
/// single spectrum.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SileroIngest {
    /// Utterance identities the Supervisor closed inside this chunk.
    pub closed: Vec<u64>,
    /// Identity of the utterance still open after this chunk.
    pub open: Option<u64>,
    /// Speech was live anywhere in this chunk — a segment is open, or one
    /// closed inside it. This is the edge bit the Apple engine lifecycle
    /// (`EpochGate`) reads instead of running a second Silero over the same
    /// PCM; it is derived from the identical two facts the ledger is minted
    /// from, in the same call, so wake/sleep and utterance identity cannot
    /// disagree about where speech was.
    pub speech_live: bool,
}

/// Supervisor-mode Silero at the Apple PCM ingress. The session's only VAD.
pub struct SileroIngress {
    session: String,
    capture_epoch: u64,
    vad: SpeechSession,
    ledger: UtteranceLedger,
}

impl SileroIngress {
    pub fn new(sample_rate: u32, session: impl Into<String>, capture_epoch: u64) -> Self {
        Self {
            session: session.into(),
            capture_epoch,
            vad: SpeechSession::new_utterance(sample_rate),
            ledger: UtteranceLedger::new(),
        }
    }

    pub fn ledger(&self) -> &UtteranceLedger {
        &self.ledger
    }

    pub fn ledger_mut(&mut self) -> &mut UtteranceLedger {
        &mut self.ledger
    }

    /// Whether Silero actually loaded. `false` means every frame reads as
    /// non-speech: no identity will ever be minted and no speech edge will ever
    /// fire, so consumers that gate on edges must fail open instead of resting
    /// forever.
    pub fn vad_available(&self) -> bool {
        self.vad.vad_available()
    }

    /// Feed one capture chunk. `samples_seen` is the session cursor *after*
    /// this chunk (same counter `apple_stream_worker` already owns).
    pub fn ingest(&mut self, samples: &[f32], samples_seen: u64) -> SileroIngest {
        if samples.is_empty() {
            return SileroIngest::default();
        }
        let events = self.vad.feed(samples, 0);
        let closed_here = events
            .iter()
            .any(|event| matches!(event, SpeechEvent::UtteranceFinal(_)));
        let open_range = self.vad.open_segment_raw_range();
        self.observe(open_range, closed_here, samples_seen)
    }

    /// The whole decision, separated from the VAD read so it is testable on
    /// synthetic edges (Silero loads from embedded bytes; a unit test that
    /// silently degraded to "no model" would prove nothing). Production calls
    /// this exactly once per chunk, from [`Self::ingest`].
    pub fn observe(
        &mut self,
        open_range: Option<(u64, u64)>,
        closed_here: bool,
        samples_seen: u64,
    ) -> SileroIngest {
        let mut out = SileroIngest {
            speech_live: closed_here || open_range.is_some(),
            ..SileroIngest::default()
        };
        if let Some((start, end)) = open_range {
            out.open =
                Some(
                    self.ledger
                        .open_or_extend(&self.session, self.capture_epoch, start, end),
                );
        }
        if closed_here && let Some(id) = self.ledger.close_open(samples_seen) {
            out.closed.push(id);
            if out.open == Some(id) {
                out.open = None;
            }
        }
        out
    }

    /// Seal any still-open Supervisor segment at capture EOF.
    pub fn flush(&mut self, samples_seen: u64) -> Option<u64> {
        let _ = self.vad.flush();
        self.ledger.close_open(samples_seen)
    }
}

/// How a Whisper window is cut relative to a Silero utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionContextMode {
    /// Audio is exactly the Silero utterance. Default.
    UtteranceOnly,
    /// Small left pad, clipped at the last long-silence fence.
    LeftAudioPad,
    /// Same audio as utterance-only; the sealed prefix is the prompt (never
    /// audio across a long silence).
    StableTextPrompt,
}

impl FusionContextMode {
    pub fn from_env() -> Self {
        match std::env::var(SILERO_FUSION_CONTEXT_ENV) {
            Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "left_pad" | "left-pad" | "pad" => Self::LeftAudioPad,
                "stable_prompt" | "stable-text" | "prompt" => Self::StableTextPrompt,
                _ => Self::UtteranceOnly,
            },
            Err(_) => Self::UtteranceOnly,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UtteranceOnly => "utterance_only",
            Self::LeftAudioPad => "left_audio_pad",
            Self::StableTextPrompt => "stable_text_prompt",
        }
    }
}

/// Cut the audio range a provider may see. Long silence is a hard fence.
pub fn bound_context_range(
    utterance: &TailSampleRange,
    last_long_silence_end: u64,
    mode: FusionContextMode,
    pad_samples: u64,
) -> TailSampleRange {
    let mut range = utterance.clone();
    if mode == FusionContextMode::LeftAudioPad {
        let want = utterance.sample_start.saturating_sub(pad_samples);
        range.sample_start = want.max(last_long_silence_end);
    }
    if range.sample_start < last_long_silence_end
        && last_long_silence_end < range.sample_end
        && last_long_silence_end > utterance.sample_start.saturating_sub(pad_samples)
    {
        // Fence is inside the requested pad — clip, never cross.
        range.sample_start = last_long_silence_end.max(utterance.sample_start);
    }
    if range.sample_start > range.sample_end {
        range.sample_start = range.sample_end;
    }
    range
}

/// One word pinned to a PCM range for fusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionWord {
    pub text: String,
    pub sample_start: u64,
    pub sample_end: u64,
}

impl FusionWord {
    pub fn from_timed(segment: &TimedTailSegment) -> Self {
        Self {
            text: segment.text.clone(),
            sample_start: segment.range.sample_start,
            sample_end: segment.range.sample_end,
        }
    }

    fn midpoint(&self) -> u64 {
        self.sample_start + (self.sample_end.saturating_sub(self.sample_start) / 2)
    }
}

/// Unresolved Apple/Whisper pair — receipt only, no confidence pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedAlternative {
    pub apple: FusionWord,
    pub whisper: FusionWord,
}

/// Conservative fusion of one unsealed utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionDecision {
    pub text: String,
    pub agreements: usize,
    pub gap_fills: usize,
    pub unresolved: Vec<UnresolvedAlternative>,
}

/// Content-free fusion receipt (no transcript text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionReceipt {
    pub utterance_id: u64,
    pub code: SkipReasonCode,
    pub agreements: usize,
    pub gap_fills: usize,
    pub unresolved: usize,
}

/// Case- and punctuation-folded token used only for agreement tests.
pub fn normalize_fusion_word(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn ranges_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
    a_start < b_end && b_start < a_end
}

/// Assign Apple words to Silero utterances by PCM overlap. Words that fall
/// in no utterance are returned as leftovers (caller receipts `no_time_overlap`).
pub fn slice_apple_words(
    ledger: &UtteranceLedger,
    words: &[FusionWord],
) -> (Vec<(u64, Vec<FusionWord>)>, Vec<FusionWord>) {
    let mut leftover = Vec::new();
    let mut by_id: std::collections::BTreeMap<u64, Vec<FusionWord>> =
        std::collections::BTreeMap::new();
    for word in words {
        match ledger.utterance_covering(word.midpoint()) {
            Some(utterance) => by_id.entry(utterance.id).or_default().push(word.clone()),
            None => leftover.push(word.clone()),
        }
    }
    (by_id.into_iter().collect(), leftover)
}

/// Conservative per-word fusion. Agreements and clear gap fills commit;
/// overlapping disagreements are receipted and Apple is kept. Confidence
/// never participates.
pub fn conservative_fuse(apple: &[FusionWord], whisper: &[FusionWord]) -> FusionDecision {
    let mut committed: Vec<FusionWord> = Vec::new();
    let mut unresolved = Vec::new();
    let mut used_whisper = vec![false; whisper.len()];
    let mut agreements = 0usize;

    for apple_word in apple {
        let overlaps: Vec<usize> = whisper
            .iter()
            .enumerate()
            .filter(|(_, whisper_word)| {
                ranges_overlap(
                    apple_word.sample_start,
                    apple_word.sample_end,
                    whisper_word.sample_start,
                    whisper_word.sample_end,
                )
            })
            .map(|(idx, _)| idx)
            .collect();
        if overlaps.is_empty() {
            committed.push(apple_word.clone());
            continue;
        }
        let apple_key = normalize_fusion_word(&apple_word.text);
        let matching: Vec<usize> = overlaps
            .iter()
            .copied()
            .filter(|&idx| normalize_fusion_word(&whisper[idx].text) == apple_key)
            .collect();
        if matching.is_empty() {
            let whisper_word = whisper[overlaps[0]].clone();
            used_whisper[overlaps[0]] = true;
            unresolved.push(UnresolvedAlternative {
                apple: apple_word.clone(),
                whisper: whisper_word,
            });
            committed.push(apple_word.clone());
        } else {
            agreements += 1;
            for idx in matching {
                used_whisper[idx] = true;
            }
            committed.push(apple_word.clone());
        }
    }

    let mut gap_fills = 0usize;
    for (idx, whisper_word) in whisper.iter().enumerate() {
        if used_whisper[idx] {
            continue;
        }
        let overlaps_apple = apple.iter().any(|apple_word| {
            ranges_overlap(
                apple_word.sample_start,
                apple_word.sample_end,
                whisper_word.sample_start,
                whisper_word.sample_end,
            )
        });
        if overlaps_apple {
            continue;
        }
        gap_fills += 1;
        committed.push(whisper_word.clone());
    }

    committed.sort_by_key(|word| word.sample_start);
    let text = committed
        .iter()
        .map(|word| word.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    FusionDecision {
        text,
        agreements,
        gap_fills,
        unresolved,
    }
}

pub fn fusion_receipt(utterance_id: u64, decision: &FusionDecision) -> FusionReceipt {
    let code = if !decision.unresolved.is_empty() {
        SkipReasonCode::UnresolvedAlternative
    } else {
        SkipReasonCode::NoTimeOverlap
    };
    FusionReceipt {
        utterance_id,
        code,
        agreements: decision.agreements,
        gap_fills: decision.gap_fills,
        unresolved: decision.unresolved.len(),
    }
}

/// One starved mid-phrase window used by the skip-table verifier.
#[cfg(test)]
#[derive(Debug, Clone)]
struct StarvedWindow {
    pub committed: &'static str,
    pub whisper: &'static str,
    pub apple: Vec<FusionWord>,
    pub whisper_words: Vec<FusionWord>,
}

/// Synthetic reconstruction of the mid-phrase-window starvation class
/// (18 skips on build 614). Baseline LCS treats head-garbage as wholesale
/// divergence; time-sliced fusion commits the overlapping agreements.
#[cfg(test)]
fn starved_mid_phrase_windows() -> Vec<StarvedWindow> {
    fn word(text: &str, start: u64, end: u64) -> FusionWord {
        FusionWord {
            text: text.to_string(),
            sample_start: start,
            sample_end: end,
        }
    }
    // 12 mid-phrase windows: Apple has the true phrase; Whisper window
    // started in babble so the LCS head is garbage, but the overlapping
    // tail agrees. 6 genuine unresolved pairs stay skipped.
    let mut windows = Vec::new();
    for i in 0..12u64 {
        let base = i * 48_000;
        windows.push(StarvedWindow {
            committed: "to jest fraza",
            whisper: "babble noise to jest fraza",
            apple: vec![
                word("to", base, base + 8_000),
                word("jest", base + 8_000, base + 16_000),
                word("fraza", base + 16_000, base + 24_000),
            ],
            whisper_words: vec![
                word("babble", base.saturating_sub(16_000), base),
                word("noise", base.saturating_sub(8_000), base),
                word("to", base, base + 8_000),
                word("jest", base + 8_000, base + 16_000),
                word("fraza", base + 16_000, base + 24_000),
            ],
        });
    }
    for i in 0..6u64 {
        let base = 600_000 + i * 16_000;
        windows.push(StarvedWindow {
            committed: "kot",
            whisper: "pies",
            apple: vec![word("kot", base, base + 8_000)],
            whisper_words: vec![word("pies", base, base + 8_000)],
        });
    }
    windows
}

/// Baseline (token LCS, fusion off) vs fusion-on skip/apply counts.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SkipTable {
    pub baseline_skips: usize,
    pub baseline_applied: usize,
    pub fusion_skips: usize,
    pub fusion_applied: usize,
}

#[cfg(test)]
impl SkipTable {
    pub fn skip_reduction_ratio(self) -> f64 {
        if self.baseline_skips == 0 {
            return 0.0;
        }
        1.0 - (self.fusion_skips as f64 / self.baseline_skips as f64)
    }
}

/// Score the starved fixture. Baseline treats any Whisper head-garbage as a
/// skip (the production change-ratio class). Fusion commits agreements +
/// gap fills and only receipts unresolved alternatives.
#[cfg(test)]
fn score_starved_fixture(windows: &[StarvedWindow]) -> SkipTable {
    let mut baseline_skips = 0usize;
    let mut baseline_applied = 0usize;
    let mut fusion_skips = 0usize;
    let mut fusion_applied = 0usize;
    for window in windows {
        let committed: Vec<&str> = window.committed.split_whitespace().collect();
        let whisper: Vec<&str> = window.whisper.split_whitespace().collect();
        let committed_in_whisper = committed
            .iter()
            .filter(|token| {
                whisper
                    .iter()
                    .any(|w| normalize_fusion_word(w) == normalize_fusion_word(token))
            })
            .count();
        let head_garbage = whisper.len() > committed.len() && committed_in_whisper < whisper.len();
        let identical = committed
            .iter()
            .zip(whisper.iter())
            .all(|(a, b)| normalize_fusion_word(a) == normalize_fusion_word(b))
            && committed.len() == whisper.len();
        if identical {
            baseline_applied += 1;
        } else if head_garbage || committed_in_whisper < committed.len() {
            baseline_skips += 1;
        } else {
            baseline_applied += 1;
        }

        let decision = conservative_fuse(&window.apple, &window.whisper_words);
        if decision.unresolved.is_empty() && (decision.agreements > 0 || decision.gap_fills > 0) {
            fusion_applied += 1;
        } else if decision.unresolved.is_empty() && decision.agreements == 0 {
            fusion_skips += 1;
        } else {
            // Unresolved alternatives are receipted, not applied as a rewrite.
            fusion_skips += 1;
        }
    }
    SkipTable {
        baseline_skips,
        baseline_applied,
        fusion_skips,
        fusion_applied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: u64, end: u64) -> FusionWord {
        FusionWord {
            text: text.to_string(),
            sample_start: start,
            sample_end: end,
        }
    }

    fn range(start: u64, end: u64) -> TailSampleRange {
        TailSampleRange {
            session: "s".into(),
            capture_epoch: 0,
            sample_start: start,
            sample_end: end,
        }
    }

    #[test]
    fn lane_defaults_off_until_operator_flip() {
        assert!(
            !lane_enabled_from_raw(None),
            "unset must keep the experimental lane off"
        );
        for off in ["0", "false", "no", "off", " OFF "] {
            assert!(
                !lane_enabled_from_raw(Some(off)),
                "{off:?} must disarm the lane"
            );
        }
        for on in ["1", "true", "yes", "on"] {
            assert!(
                lane_enabled_from_raw(Some(on)),
                "{on:?} must explicitly arm the lane"
            );
        }
    }

    /// The unification claim, stated as a test: **one** observation of the
    /// spectrum produces both the ledger identity and the lifecycle edge bit.
    /// Two speech segments split by a closing edge mint two identities, and the
    /// `speech_live` the epoch gate reads is true exactly across those two
    /// segments and false in the silence between them.
    #[test]
    fn one_observation_feeds_both_identity_and_the_lifecycle_edge() {
        let mut ingress = SileroIngress::new(16_000, "s", 0);

        // Segment 1: open at 0, still open, then close inside the third chunk.
        let a = ingress.observe(Some((0, 8_000)), false, 8_000);
        assert_eq!(a.open, Some(1));
        assert!(a.speech_live, "an open segment is a live speech edge");
        let b = ingress.observe(Some((0, 16_000)), false, 16_000);
        assert_eq!(b.open, Some(1), "an extending segment keeps its identity");
        let close = ingress.observe(None, true, 24_000);
        assert_eq!(close.closed, vec![1]);
        assert!(
            close.speech_live,
            "the chunk a segment closes in is still speech — the silence \
             counter starts after Silero's own hysteresis, never before it"
        );

        // Long silence: no edge, no identity.
        for cursor in [32_000u64, 40_000, 48_000] {
            let quiet = ingress.observe(None, false, cursor);
            assert!(!quiet.speech_live, "silence is not a speech edge");
            assert!(quiet.closed.is_empty());
            assert_eq!(quiet.open, None);
        }

        // Segment 2 past the long-silence fence: a NEW identity, not an extend.
        let fence = (LONG_SILENCE_FENCE_SECS * 16_000.0) as u64;
        let second_start = 24_000 + fence + 8_000;
        let c = ingress.observe(Some((second_start, second_start + 8_000)), false, 56_000);
        assert_eq!(
            c.open,
            Some(2),
            "speech after a closing edge mints a second utterance"
        );
        assert!(c.speech_live);

        let ledger = ingress.ledger();
        assert_eq!(ledger.utterances().len(), 2);
        assert_eq!(ledger.utterances()[0].range.sample_start, 0);
        assert_eq!(ledger.utterances()[0].range.sample_end, 24_000);
        assert!(ledger.utterances()[0].closed);
        assert_eq!(ledger.utterances()[1].range.sample_start, second_start);
        assert!(!ledger.utterances()[1].closed);
        assert!(
            ledger.utterances()[1].range.sample_start - ledger.utterances()[0].range.sample_end
                >= fence,
            "fixture must actually clear the long-silence fence"
        );
    }

    /// Enclosure, not overlap: a span may only adopt a Silero range that
    /// already covers every sample it claimed.
    #[test]
    fn enclosure_is_required_before_a_span_adopts_a_silero_range() {
        let mut ledger = UtteranceLedger::new();
        ledger.open_or_extend("s", 0, 10_000, 30_000);
        ledger.close_open(30_000);

        let enclosed = ledger
            .utterance_enclosing(12_000, 20_000)
            .expect("a span inside the edge binds to it");
        assert_eq!(enclosed.id, 1);
        assert_eq!(enclosed.range.sample_start, 10_000);
        assert_eq!(enclosed.range.sample_end, 30_000);

        assert!(
            ledger.utterance_enclosing(5_000, 20_000).is_none(),
            "a span starting before the edge must NOT adopt it"
        );
        assert!(
            ledger.utterance_enclosing(20_000, 40_000).is_none(),
            "a span ending after the edge must NOT adopt it"
        );
        assert!(
            ledger.utterance_enclosing(80_000, 90_000).is_none(),
            "no edge at all is fail-open, not a panic"
        );
    }

    /// One ledger, one id space: an id burnt by the Apple-boundary fallback is
    /// never re-minted for a real utterance.
    #[test]
    fn reserved_ids_are_never_reused_by_a_minted_utterance() {
        let mut ledger = UtteranceLedger::new();
        assert_eq!(ledger.reserve_id(), 1);
        assert_eq!(ledger.reserve_id(), 2);
        assert_eq!(
            ledger.open_or_extend("s", 0, 0, 1_000),
            3,
            "minting must continue past every reserved id"
        );
        assert_eq!(ledger.utterances().len(), 1, "a reservation is not a span");
    }

    #[test]
    fn w13_fusion_conservative_commits_agreements() {
        let apple = vec![
            word("the", 0, 8_000),
            word("cat", 8_000, 16_000),
            word("sat", 16_000, 24_000),
        ];
        let whisper_agree = vec![
            word("the", 0, 8_000),
            word("cat", 8_000, 16_000),
            word("sat", 16_000, 24_000),
        ];
        let agreed = conservative_fuse(&apple, &whisper_agree);
        assert_eq!(agreed.text, "the cat sat");
        assert_eq!(agreed.agreements, 3);
        assert_eq!(agreed.gap_fills, 0);
        assert!(agreed.unresolved.is_empty());

        let mut whisper_gap = whisper_agree.clone();
        whisper_gap.push(word("here", 24_000, 32_000));
        let filled = conservative_fuse(&apple, &whisper_gap);
        assert_eq!(filled.text, "the cat sat here");
        assert_eq!(filled.agreements, 3);
        assert_eq!(filled.gap_fills, 1);
        assert!(filled.unresolved.is_empty());

        let whisper_conflict = vec![
            word("the", 0, 8_000),
            word("dog", 8_000, 16_000),
            word("sat", 16_000, 24_000),
        ];
        let conflicted = conservative_fuse(&apple, &whisper_conflict);
        assert_eq!(conflicted.text, "the cat sat");
        assert_eq!(conflicted.agreements, 2);
        assert_eq!(conflicted.unresolved.len(), 1);
        assert_eq!(conflicted.unresolved[0].apple.text, "cat");
        assert_eq!(conflicted.unresolved[0].whisper.text, "dog");
        let receipt = fusion_receipt(7, &conflicted);
        assert_eq!(receipt.code, SkipReasonCode::UnresolvedAlternative);
        assert_eq!(receipt.unresolved, 1);
    }

    #[test]
    fn apple_words_slice_onto_silero_edges() {
        let mut ledger = UtteranceLedger::new();
        ledger.open_or_extend("s", 0, 0, 24_000);
        ledger.close_open(24_000);
        ledger.open_or_extend("s", 0, 32_000, 48_000);
        let words = vec![
            word("alpha", 1_000, 8_000),
            word("beta", 33_000, 40_000),
            word("orphan", 80_000, 88_000),
        ];
        let (sliced, leftover) = slice_apple_words(&ledger, &words);
        assert_eq!(sliced.len(), 2);
        assert_eq!(sliced[0].1[0].text, "alpha");
        assert_eq!(sliced[1].1[0].text, "beta");
        assert_eq!(leftover.len(), 1);
        assert_eq!(leftover[0].text, "orphan");
    }

    #[test]
    fn left_pad_never_crosses_long_silence() {
        let utterance = range(48_000, 64_000);
        let silence_end = 40_000;
        let padded = bound_context_range(
            &utterance,
            silence_end,
            FusionContextMode::LeftAudioPad,
            16_000,
        );
        assert_eq!(padded.sample_start, silence_end);
        assert_eq!(padded.sample_end, 64_000);

        let utterance_only = bound_context_range(
            &utterance,
            silence_end,
            FusionContextMode::UtteranceOnly,
            16_000,
        );
        assert_eq!(utterance_only.sample_start, 48_000);

        let prompt = bound_context_range(
            &utterance,
            silence_end,
            FusionContextMode::StableTextPrompt,
            16_000,
        );
        assert_eq!(prompt.sample_start, 48_000);
    }

    #[test]
    fn w13_fusion_starved_fixture_skip_table() {
        let windows = starved_mid_phrase_windows();
        assert_eq!(windows.len(), 18);
        let table = score_starved_fixture(&windows);
        println!(
            "starved fixture skip table: baseline skips={} applied={} | fusion skips={} applied={} | reduction={:.0}%",
            table.baseline_skips,
            table.baseline_applied,
            table.fusion_skips,
            table.fusion_applied,
            table.skip_reduction_ratio() * 100.0
        );
        assert!(
            table.skip_reduction_ratio() + f64::EPSILON >= 0.50,
            "skip reduction {:.2} < 50% (baseline {} → fusion {})",
            table.skip_reduction_ratio(),
            table.baseline_skips,
            table.fusion_skips
        );
        assert!(
            table.fusion_applied >= table.baseline_applied,
            "applied dropped: baseline {} fusion {}",
            table.baseline_applied,
            table.fusion_applied
        );
    }
}
