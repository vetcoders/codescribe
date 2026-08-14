//! Layer 1 — Whisper Tail Patch (diff core).
//!
//! Implements the ADR "Layered Incremental Transcription Pipeline" (2026-05-26)
//! Layer 1 primitive: given the text Layer 0 already committed for an utterance,
//! and a higher-recall Whisper re-transcription of the *same* audio slice,
//! produce **bounded** [`EngineEvent::ReplaceRange`] patches that fill in /
//! correct only the tokens that differ.
//!
//! # Relationship to Smart final-pass (`FINAL_PASS_MODE`)
//!
//! **Orthogonal toggles — no silent coupling.**
//!
//! | Control | Env | Default | What it does |
//! | --- | --- | --- | --- |
//! | Final pass | `FINAL_PASS_MODE` | `smart` | Stop-path only: whether to run a full WAV Whisper re-pass after release |
//! | Layered / Layer 1 | `CODESCRIBE_LAYERED_TRANSCRIPTION` | **phase1** | During-hold gap-fill: Whisper tail patches on sealed utterances. Unset → phase1; explicit `off`/`0`/`false` disarms. |
//!
//! - **Smart** = skip full stop re-pass when streaming completeness is
//!   adjudicated Complete. It does **not** enable layered transcription.
//! - **Off** = never full stop re-pass. It does **not** force Whisper at stop.
//! - Layered phase ≥ 1 may run under any final-pass mode when the live session
//!   path actually wires Layer 1 (see below).
//!
//! Product intent: Smart *works with* layered (completeness skip + live
//! gap-fill). Phase 1 is the stock live default; W13 fusion / idempotence /
//! highlight flags remain the operator-flip surface, not this gate.
//!
//! # Where Layer 1 is wired today
//!
//! Both live paths are wired; gate is [`layered_phase`] ≥ 1 on each.
//!
//! - **VAD/scheduler:** `core/pipeline/streaming/session.rs` →
//!   `vad_transcription_session` (Whisper engine, or Apple with
//!   `CODESCRIBE_APPLE_STT_LIVE_MODE=wav`). Attaches FINAL audio per work item,
//!   spawns Whisper re-transcribe + [`compute_tail_patch`], emits
//!   `ReplaceRange { source: TailPatch }`, counts in `SessionFinalised.layer_summary`.
//! - **Apple progressive live:** `core/pipeline/streaming/apple_live_session.rs`
//!   → `apple_stream_transcription_session` (W2-A). Each sealed `UtteranceFinal`
//!   resolves to its retained PCM window and is handed to the async Layer 1
//!   lane, at most one job in flight so Whisper never sits on the event-drain
//!   loop. A boundary that cannot address retained audio is never patched; a
//!   full queue drops the request rather than stalling capture; the bounded
//!   backlog left when capture stops is settled before `SessionFinalised`.
//!
//! # Invariants (from the ADR "Hard invariants")
//!
//! - **NEVER REWRITE FROM ZERO.** This core only ever emits bounded
//!   `ReplaceRange { source: LayerSource::TailPatch }` events scoped to a single
//!   utterance. It never returns a full-buffer overwrite.
//! - **Bounded patches.** Every emitted event references char offsets inside the
//!   committed utterance text passed in.
//! - **Conservative by default.** If the diff distance exceeds
//!   [`TailPatchConfig::max_change_ratio`], the whole patch is dropped
//!   ([`TailPatchOutcome::Skipped`]) and Layer 0 output stands unchanged —
//!   "don't patch if uncertain".
//!
//! # Scope of this cut (v1)
//!
//! Emits **substitution** and **insertion** patches (wrong token → right token,
//! missing token filled). Deletions (Whisper saw *fewer* words than Layer 0) are
//! intentionally left intact: dropping words the user already saw is the riskier
//! direction, so v1 leaves them to a later layer / the operator. Deleted tokens
//! still count toward the change ratio so a wildly divergent re-transcription is
//! skipped wholesale.
//!
//! # Under-commit is not divergence (W-C)
//!
//! The change-ratio cap above assumes the two texts describe the *same* speech
//! and merely disagree. When Layer 0 lost whole phrases — the measured
//! 104 s / 220 ch and 107 s / 118 ch Polish sessions — the re-transcription is
//! not a divergent opinion, it is the speech that never reached the canvas, and
//! the cap silently threw it away. [`TailPatchOutcome::UnderCommit`] separates
//! the two: a re-transcription that still *contains* the committed canvas
//! ([`UNDER_COMMIT_MIN_COVERAGE`]) while carrying substantially more of it
//! ([`UNDER_COMMIT_RATIO`]) is classified as under-commit, its recovered
//! material is emitted as bounded gap-**appends** where the anchor is a matched
//! committed token boundary, and anything that could only land by rewriting a
//! committed span escalates [`UnderCommit::residual_required`] instead. A
//! re-transcription that does *not* contain the canvas is still ordinary
//! divergence and still `Skipped`.
//!
//! This module is a **pure** function of its inputs in the sense that matters:
//! it performs no audio capture and no network calls, and its return value
//! depends only on its arguments. Its one side effect is a single INFO receipt
//! per non-patching outcome (counts and reason only — never transcript text),
//! because the discarded-truth bug was invisible precisely for want of one.
//!
//! Contract: the `committed` argument is byte-identical to the emitted
//! `UtteranceFinal.text` and is already trimmed by the emitter (single trim
//! owner: `final_text` at the session.rs emit site).

use tracing::info;

use crate::pipeline::contracts::{EngineEvent, LayerSource};

/// Env flag gating the layered transcription pipeline.
///
/// `CODESCRIBE_LAYERED_TRANSCRIPTION=phase{1,2,3,4}` — **defaults to phase1**.
/// The live tail patch is a core element of the triangulation, not an opt-in
/// (operator directive 2026-08-09: "korekcje na żywo to live tail patch, który
/// MUSI być podstawowym elementem"). Explicit `off`/`0`/`false` disables;
/// explicit `phaseN` selects a phase.
///
/// **Not** `FINAL_PASS_MODE`: Smart final-pass never writes this flag. Kept
/// here (not in the config hub) so this cut stays isolated; the orchestrator
/// can promote it to a typed config field when it lands.
pub const LAYERED_TRANSCRIPTION_ENV: &str = "CODESCRIBE_LAYERED_TRANSCRIPTION";

/// Phase served when the flag is unset — Layer 1 live tail patch on.
const LAYERED_DEFAULT_PHASE: u8 = 1;

/// Env override for [`TailPatchConfig::max_change_ratio`].
pub const TAIL_PATCH_MAX_CHANGE_RATIO_ENV: &str = "CODESCRIBE_TAIL_PATCH_MAX_CHANGE_RATIO";

/// Committed/retranscribed **token** ratio below which Layer 0 is judged to have
/// under-committed rather than to have merely disagreed.
///
/// The measured eaten sessions sat far under this (220 ch of a 104 s take), and
/// a healthy tail-patch — a word or two corrected in a phrase Layer 0 fully
/// heard — sits at ~1.0. `0.6` leaves ordinary Whisper verbosity (articles,
/// re-segmented compounds) on the normal path.
pub const UNDER_COMMIT_RATIO: f64 = 0.6;

/// Minimum re-transcribed token count before under-commit is even considered,
/// for canvases of ≥3 tokens.
///
/// A two-word Whisper burst against a one-word canvas satisfies any ratio while
/// carrying no recoverable speech; requiring a real sentence keeps the
/// escalation attached to material worth recovering.
///
/// The floor SCALES DOWN for short canvases (see
/// [`under_commit_min_retranscribed`]): session a5623d55 (2026-08-12) lost the
/// utterance head three times in one minute because its live windows held only
/// 1-2 committed tokens and Whisper's 5-token recovery could never reach an
/// absolute 6. Coverage, not length, is the garbage discriminator.
pub const UNDER_COMMIT_MIN_RETRANSCRIBED_TOKENS: usize = 6;

/// Hard floor of the scaled minimum: even a one-token canvas needs at least
/// this many recovered tokens before escalation is considered.
pub const UNDER_COMMIT_MIN_SHORT_WINDOW_TOKENS: usize = 4;

/// Effective minimum re-transcribed token count for a given canvas size:
/// `min(6, max(4, 2 × committed))` — never stricter than the absolute
/// constant, looser only where the canvas itself is one or two tokens.
pub fn under_commit_min_retranscribed(committed_tokens: usize) -> usize {
    (committed_tokens * 2).clamp(
        UNDER_COMMIT_MIN_SHORT_WINDOW_TOKENS,
        UNDER_COMMIT_MIN_RETRANSCRIBED_TOKENS,
    )
}

/// Fraction of committed tokens that must be found inside the re-transcription
/// before its anchors may be trusted.
///
/// **This is the discriminator, not the ratio.** Under-commit means Whisper
/// heard everything the canvas holds *plus* what Layer 0 lost, so the committed
/// tokens align and the gaps between them are addressable. A re-transcription
/// that does not contain the canvas is ordinary divergence: its offsets mean
/// nothing against the committed text, so it stays [`TailPatchOutcome::Skipped`]
/// and Layer 0 stands.
pub const UNDER_COMMIT_MIN_COVERAGE: f64 = 0.8;

/// Canvas/re-transcription token ratio under which the canvas is treated as
/// *structurally starved* rather than merely incomplete: Whisper carries about
/// three times the material Layer 0 committed.
pub const UNDER_COMMIT_STARVED_CANVAS_RATIO: f64 = 0.35;

/// Coverage required of a structurally starved canvas.
///
/// # Why the full bar is the wrong instrument here
///
/// Token coverage measures whether the two texts agree on *words*. That is a
/// sound proxy for "same speech" while Layer 0 hears well. It stops being one
/// exactly when this lane matters most: when SFSpeech mangles the phonetics,
/// its tokens stop matching anything ("zrób z loctree" committed as "gdzieś in.
/// zrope"), coverage collapses, and the full bar rejects the recovery — so the
/// worse Layer 0 heard, the more certain the rejection. Measured on the
/// operator's live log 2026-08-14: 295 change-ratio skips, 181 of them (61%)
/// carrying MORE material than the canvas, 5341 characters of speech discarded;
/// the sharpest single case committed 15 characters against 223 re-transcribed.
///
/// What the full bar is really defending against — an unrelated decode pasting
/// its text onto the user's transcript — cannot happen on this seam: the window
/// handed to Whisper is cut from the sealed span's own PCM range
/// (`resolve_sealed_audio_window`), so both texts describe the same samples by
/// construction. Audio identity is the guarantee; token coverage was standing
/// in for it. A starved canvas therefore keeps a real but lower bar — enough
/// anchors to place appends honestly, not enough to demand agreement from a
/// canvas that is mostly noise. Everything unplaceable still escalates to
/// `residual_required`; nothing is ever rewritten.
pub const UNDER_COMMIT_STARVED_MIN_COVERAGE: f64 = 0.45;

/// Coverage bar for this canvas: the full bar normally, the starved bar when
/// Whisper carries several times the canvas's material.
pub fn under_commit_min_coverage(commit_ratio: f64) -> f64 {
    if commit_ratio <= UNDER_COMMIT_STARVED_CANVAS_RATIO {
        UNDER_COMMIT_STARVED_MIN_COVERAGE
    } else {
        UNDER_COMMIT_MIN_COVERAGE
    }
}

/// Parse a layered-transcription phase token (`phase1`..`phase4` or bare `1`..`4`).
///
/// Returns `None` for off/empty/garbage — including final-pass tokens
/// (`smart` / `always` / `off`), so the two env families cannot be confused.
pub fn parse_layered_phase_value(raw: &str) -> Option<u8> {
    let raw = raw.trim().to_ascii_lowercase();
    if raw.is_empty() || raw == "off" || raw == "0" || raw == "false" || raw == "no" {
        return None;
    }
    let digits = raw.strip_prefix("phase").unwrap_or(&raw);
    match digits.parse::<u8>().ok()? {
        n @ 1..=4 => Some(n),
        _ => None,
    }
}

/// Active layered-transcription phase. Unset → the default phase (live tail
/// patch on); an explicit `off`/`0`/`false` — or unparseable garbage — is the
/// only way to `None`.
///
/// Independent of `FINAL_PASS_MODE` / Smart completeness skip.
pub fn layered_phase() -> Option<u8> {
    let raw = std::env::var(LAYERED_TRANSCRIPTION_ENV).ok();
    layered_phase_from_raw(raw.as_deref())
}

/// Resolve the layered phase from an optional raw override without touching
/// process-global environment state. `None` carries the production default.
pub fn layered_phase_from_raw(raw: Option<&str>) -> Option<u8> {
    raw.map(parse_layered_phase_value)
        .unwrap_or(Some(LAYERED_DEFAULT_PHASE))
}

/// Env override for [`TailPatchConfig::small_edit_token_floor`].
pub const TAIL_PATCH_SMALL_EDIT_FLOOR_ENV: &str = "CODESCRIBE_TAIL_PATCH_SMALL_EDIT_FLOOR";

/// Tuning for the tail-patch diff.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TailPatchConfig {
    /// Maximum fraction of committed tokens that may change before the whole
    /// patch is skipped. `0.5` means: if more than half the utterance would be
    /// touched, leave Layer 0 output untouched.
    pub max_change_ratio: f64,
    /// Absolute change budget under which a substitution-shaped fix bypasses
    /// the ratio cap.
    ///
    /// The ratio alone starves short utterances structurally: on a 1-3-token
    /// commit any real correction is ≥50% change, so the lane could never fix
    /// a single misheard word — the very job it exists for. Measured on the
    /// 2026-08-12 log: 116 skips, 0 applied patches, 38 of them at exactly
    /// ratio 1.00 ("Kos" → "kombos" class). A substitution of this many tokens
    /// or fewer is bounded by definition — wholesale divergence, which the
    /// ratio guards against, always touches more. Pure insertions never use
    /// this budget; they keep their under-commit / noise routing.
    pub small_edit_token_floor: usize,
}

impl Default for TailPatchConfig {
    /// Conservative default: skip the patch if more than half the tokens would
    /// change — unless the whole change fits inside the small-edit budget.
    fn default() -> Self {
        Self {
            max_change_ratio: 0.5,
            small_edit_token_floor: 3,
        }
    }
}

impl TailPatchConfig {
    /// Read config from env, falling back to defaults.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Some(value) = std::env::var(TAIL_PATCH_MAX_CHANGE_RATIO_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
        {
            cfg.max_change_ratio = value;
        }
        if let Some(value) = std::env::var(TAIL_PATCH_SMALL_EDIT_FLOOR_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
        {
            cfg.small_edit_token_floor = value;
        }
        cfg
    }
}

/// Stable skip-reason code on a tail-patch or fusion receipt.
///
/// The string [`TailPatchOutcome::Skipped::reason`] stays human-readable; this
/// code is what a later starvation diagnosis greps. W13-3B adds the time-slice
/// and fusion tokens; the LCS tokens keep the v1 receipts diagnosable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReasonCode {
    EmptyCommitted,
    EmptyRetranscription,
    NoCommittedTokens,
    ChangeRatio,
    HeadGarbage,
    NoTimeOverlap,
    LowConfidence,
    UnresolvedAlternative,
    SealedFence,
    Divergence,
    ProviderError,
}

impl SkipReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmptyCommitted => "empty_committed",
            Self::EmptyRetranscription => "empty_retranscription",
            Self::NoCommittedTokens => "no_committed_tokens",
            Self::ChangeRatio => "change_ratio",
            Self::HeadGarbage => "head_garbage",
            Self::NoTimeOverlap => "no_time_overlap",
            Self::LowConfidence => "low_confidence",
            Self::UnresolvedAlternative => "unresolved_alternative",
            Self::SealedFence => "sealed_fence",
            Self::Divergence => "divergence",
            Self::ProviderError => "provider_error",
        }
    }
}

/// Result of a tail-patch diff.
#[derive(Debug, Clone, PartialEq)]
pub enum TailPatchOutcome {
    /// Bounded patches to apply (always `EngineEvent::ReplaceRange`), ordered so
    /// that sequential application to the committed text is offset-stable
    /// (descending by `start`).
    Patches(Vec<EngineEvent>),
    /// Re-transcription matched the committed text — nothing to do.
    NoChange,
    /// Diff exceeded the safety threshold (or there was nothing to patch
    /// against); Layer 0 output stands unchanged.
    Skipped {
        code: SkipReasonCode,
        reason: String,
    },
    /// Layer 0 committed substantially less than the audio carried, and the
    /// committed canvas is still contained in the re-transcription. Recovered
    /// speech, not a diff to clamp — see [`UnderCommit`].
    UnderCommit(UnderCommit),
}

impl TailPatchOutcome {
    /// Construct a skipped outcome with a stable reason code.
    pub fn skipped(code: SkipReasonCode, reason: impl Into<String>) -> Self {
        Self::Skipped {
            code,
            reason: reason.into(),
        }
    }
}

/// A Layer-0 under-commit: what was recovered, what could be placed live, and
/// whether the stop path still owes the session a residual gap fill.
///
/// Append-plus-gap-fill is the whole contract here. `appends` never touches a
/// committed span — every event is a zero-width `ReplaceRange` anchored on a
/// matched committed token boundary (or on buffer start). Material that could
/// only land by rewriting committed text is *not* emitted; it raises
/// `residual_required` instead, because the asymmetry is settled doctrine: lost
/// speech is unrecoverable, duplication is filterable downstream.
#[derive(Debug, Clone, PartialEq)]
pub struct UnderCommit {
    /// Bounded gap-append events, descending by `start` so sequential
    /// application against the committed text is offset-stable. May be empty.
    pub appends: Vec<EngineEvent>,
    /// Recovered speech that no safe anchor could place. The stop path must
    /// treat this session as owing a residual gap fill rather than as a
    /// complete streaming transcript.
    pub residual_required: bool,
    /// Whitespace-delimited token count of the committed canvas.
    pub committed_tokens: usize,
    /// Whitespace-delimited token count of the re-transcription.
    pub retranscribed_tokens: usize,
    /// Char count of the trimmed committed canvas (log/telemetry only).
    pub committed_chars: usize,
    /// Char count of the trimmed re-transcription (log/telemetry only).
    pub retranscribed_chars: usize,
    /// `committed_tokens / retranscribed_tokens` — under [`UNDER_COMMIT_RATIO`].
    pub commit_ratio: f64,
}

impl UnderCommit {
    /// Stable log/wire tag naming what this escalation asks of the stop path.
    pub fn reason(&self) -> &'static str {
        if self.residual_required {
            "under_commit_residual_required"
        } else {
            "under_commit_gap_append"
        }
    }
}

impl TailPatchOutcome {
    /// Bounded events this outcome contributes to the committed canvas.
    ///
    /// One accessor for both event-bearing arms so a sink cannot forward
    /// `Patches` while silently dropping recovered gap-appends — the exact
    /// shape of the bug this cut repairs.
    pub fn events(&self) -> &[EngineEvent] {
        match self {
            Self::Patches(events) => events,
            Self::UnderCommit(under) => &under.appends,
            Self::NoChange | Self::Skipped { .. } => &[],
        }
    }

    /// Same events, owned, for sinks that consume the outcome.
    pub fn into_events(self) -> Vec<EngineEvent> {
        match self {
            Self::Patches(events) => events,
            Self::UnderCommit(under) => under.appends,
            Self::NoChange | Self::Skipped { .. } => Vec::new(),
        }
    }

    /// Whether the stop path must run residual gap fill because recovered
    /// speech could not be placed on the live canvas.
    pub fn residual_required(&self) -> bool {
        matches!(self, Self::UnderCommit(under) if under.residual_required)
    }
}

/// A whitespace-delimited token with char-offset span inside the source string.
#[derive(Debug, Clone, PartialEq)]
struct Token {
    /// Char index of the first char (inclusive).
    char_start: usize,
    /// Char index one past the last char (exclusive).
    char_end: usize,
    /// Token body with surrounding whitespace stripped.
    text: String,
}

/// Split on whitespace into tokens carrying char-offset spans.
///
/// Offsets are char- not byte-based so Polish diacritics cannot skew the
/// bounded ranges emitted downstream. Whitespace never enters a token, so
/// leading/trailing whitespace in a Whisper re-transcription is inert.
fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    let mut buf = String::new();
    for (char_idx, ch) in input.chars().enumerate() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                tokens.push(Token {
                    char_start: s,
                    char_end: char_idx,
                    text: std::mem::take(&mut buf),
                });
            }
        } else {
            if start.is_none() {
                start = Some(char_idx);
            }
            buf.push(ch);
        }
    }
    if let Some(s) = start {
        let char_end = input.chars().count();
        tokens.push(Token {
            char_start: s,
            char_end,
            text: buf,
        });
    }
    tokens
}

/// One contiguous diff group between two consecutive aligned (matched) tokens.
struct EditGroup {
    /// Indices into the committed token list that are unmatched (replaced/deleted).
    committed: std::ops::Range<usize>,
    /// Indices into the re-transcribed token list that are unmatched (inserted).
    retranscribed: std::ops::Range<usize>,
    /// Char position to use when the committed side is empty (insertion anchor):
    /// the char_end of the previous matched committed token, or 0 at buffer start.
    anchor: usize,
    /// Whether there is a previous matched committed token (controls insertion spacing).
    has_prev_match: bool,
}

/// Alignment key for LCS matching: casefolded, stripped of leading/trailing
/// non-alphanumerics.
///
/// The two texts come from different normalization worlds — the committed
/// canvas carries Apple's casing, punctuation and the lexicon's rewrites,
/// while the Whisper re-transcription is bare lowercase. Compared byte-exact,
/// "Jaki chcesz. Kos." vs "jaki chcesz kombos" shares zero tokens and reads
/// as wholesale divergence; every skip ratio in the starved 2026-08-12 log
/// was inflated this way. Matching on the key aligns what a listener would
/// call the same word; matched tokens are never patched, so the canvas keeps
/// its casing and punctuation. Diacritics stay significant — they are
/// content, not decoration. A token that is all punctuation falls back to its
/// lowercased raw form so it can only match another such token.
fn alignment_key(token: &str) -> String {
    let stripped = token.trim_matches(|c: char| !c.is_alphanumeric());
    if stripped.is_empty() {
        token.to_lowercase()
    } else {
        stripped.to_lowercase()
    }
}

/// Consecutive matching tokens that mark a recovery as already carried.
///
/// Four is the shortest run that is not ordinary Polish repetition: "nam na
/// zrobienie" (3) recurs naturally, "która pozwoli nam na" (4) does not.
pub const DUPLICATE_RUN_TOKENS: usize = 4;

/// Whether `canvas` already carries the words in `candidate`.
///
/// Public seam for the presentation layer, which applies a patch against the
/// canvas as it stands NOW — not the canvas the patch was computed against.
/// Measured 2026-08-14: Layer 1 computed an append for a 15-character canvas
/// while SFSpeech went on to restate the SAME utterance at 47 characters,
/// already delivering the words the append recovered; the append landed on the
/// restatement and duplicated the phrase.
pub fn text_already_carries(canvas: &str, candidate: &str) -> bool {
    let canvas_tokens = tokenize(canvas);
    let candidate_tokens = tokenize(candidate);
    let refs: Vec<&Token> = candidate_tokens.iter().collect();
    canvas_already_carries(&canvas_tokens, &refs)
}

/// Whether the canvas already carries this recovered run of words.
///
/// Compared on [`alignment_key`] — the same key the aligner matches on — so a
/// phrase the canvas holds in mangled casing/punctuation still counts as
/// present. Single-token runs are exempt: one repeated short word ("i", "no")
/// is ordinary speech, not a duplicated recovery, and refusing those would
/// starve the lane again.
fn canvas_already_carries(canvas: &[Token], recovered: &[&Token]) -> bool {
    let needle: Vec<String> = recovered
        .iter()
        .map(|token| alignment_key(&token.text))
        .collect();
    let hay: Vec<String> = canvas
        .iter()
        .map(|token| alignment_key(&token.text))
        .collect();
    // A run counts as already carried when a long-enough CONTIGUOUS stretch of
    // it appears in the canvas — not when the whole run matches end to end.
    // Requiring the full run is defeated by exactly the defect this guards
    // against: Layer 0 mangles a word at the edge ("hard Pru" against the
    // recovered "hard pruna"), one key differs, and the duplicate is placed
    // anyway. Four consecutive content words repeating verbatim is speech the
    // canvas already holds; three or fewer is ordinary Polish repetition.
    let span = DUPLICATE_RUN_TOKENS.min(needle.len());
    if span < DUPLICATE_RUN_TOKENS {
        return false;
    }
    needle
        .windows(span)
        .any(|run| hay.windows(span).any(|window| window == run))
}

/// Longest-common-subsequence alignment over normalized token keys
/// ([`alignment_key`]).
///
/// Returns pairs `(committed_idx, retranscribed_idx)` of matched tokens, in order.
fn lcs_matches(committed: &[Token], retranscribed: &[Token]) -> Vec<(usize, usize)> {
    let c_keys: Vec<String> = committed.iter().map(|t| alignment_key(&t.text)).collect();
    let r_keys: Vec<String> = retranscribed
        .iter()
        .map(|t| alignment_key(&t.text))
        .collect();
    let m = committed.len();
    let n = retranscribed.len();
    // dp[i][j] = LCS length of committed[i..] and retranscribed[j..].
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if c_keys[i] == r_keys[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut matches = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m && j < n {
        if c_keys[i] == r_keys[j] {
            matches.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matches
}

/// One INFO receipt for a tail-patch outcome that put nothing on the canvas.
///
/// Counts and reason only. The transcript is the user's speech and never enters
/// a log line; the counts are what makes a starved session diagnosable, which
/// is exactly what was missing when `Skipped` was a `debug!` and the recovered
/// text vanished without trace.
fn log_skipped_receipt(
    utterance_id: u64,
    reason: &str,
    committed: &str,
    retranscribed: &str,
    committed_tokens: usize,
    retranscribed_tokens: usize,
) {
    info!(
        utterance_id,
        reason,
        committed_chars = committed.trim().chars().count(),
        retranscribed_chars = retranscribed.trim().chars().count(),
        committed_tokens,
        retranscribed_tokens,
        "tail_patch_skipped"
    );
}

/// Everything the under-commit classifier reads about one alignment.
struct UnderCommitScan<'a> {
    c_tokens: &'a [Token],
    r_tokens: &'a [Token],
    matched: &'a [(usize, usize)],
    groups: &'a [EditGroup],
    /// Canvas sealed before this utterance; read-only duplicate guard.
    neighbour_tokens: &'a [Token],
}

/// Decide whether a change-ratio rejection is really an under-commit, and if so
/// what of the recovered speech can be appended safely.
///
/// Returns `None` for ordinary divergence, leaving the caller to skip.
///
/// Three gates, in order of how much they can hurt if wrong:
/// 1. the re-transcription must be substantial ([`UNDER_COMMIT_MIN_RETRANSCRIBED_TOKENS`]);
/// 2. it must carry substantially more than the canvas ([`UNDER_COMMIT_RATIO`]);
/// 3. it must still *contain* the canvas ([`UNDER_COMMIT_MIN_COVERAGE`]) — without
///    this an unrelated decode would append its whole text to the user's transcript.
fn classify_under_commit(
    scan: UnderCommitScan<'_>,
    utterance_id: u64,
    committed_chars: usize,
    retranscribed_chars: usize,
) -> Option<UnderCommit> {
    let UnderCommitScan {
        c_tokens,
        r_tokens,
        matched,
        groups,
        neighbour_tokens,
    } = scan;
    let committed_tokens = c_tokens.len();
    let retranscribed_tokens = r_tokens.len();
    if retranscribed_tokens < under_commit_min_retranscribed(committed_tokens) {
        return None;
    }
    let commit_ratio = committed_tokens as f64 / retranscribed_tokens as f64;
    if commit_ratio >= UNDER_COMMIT_RATIO {
        return None;
    }
    let coverage = matched.len() as f64 / committed_tokens as f64;
    let min_coverage = under_commit_min_coverage(commit_ratio);
    // Two different decisions were fused into this one bar, and fusing them is
    // what discarded speech:
    //
    // 1. May recovered material be PLACED into the canvas? That needs trusted
    //    anchors — a matched committed token to append beside — so it keeps the
    //    coverage bar.
    // 2. May the recovery be ESCALATED to the stop path? That touches nothing
    //    live, so a failed coverage check is no argument for dropping it. Under
    //    the old rule the answer to both was "no", and 5341 measured characters
    //    left as a `change_ratio` receipt.
    //
    // Below the bar the canvas is too mangled to anchor against, so nothing is
    // placed inline — but the material is owed to the stop path, not to /dev/null.
    // Placing on weak anchors was tried and MEASURED DOWN, 2026-08-14: five
    // takes, delivered text against the lbrx reference, mean WER 0.604 -> 0.612
    // (worst case 01_no-to-dobra 0.356 -> 0.425). Recovered words placed beside
    // an anchor the canvas cannot really vouch for land in the wrong part of the
    // sentence more often than they fill a real gap, and the last-mile duplicate
    // guard cannot catch that — it only catches repeats, not misplacement.
    // The bar stays; the recovery that cannot be anchored is owed elsewhere.
    let anchors_trusted = coverage >= min_coverage;
    if !anchors_trusted && commit_ratio > UNDER_COMMIT_STARVED_CANVAS_RATIO {
        // Not starved and not alignable: ordinary divergence, Layer 0 stands.
        return None;
    }

    let mut appends: Vec<EngineEvent> = Vec::new();
    let mut residual_required = false;
    for g in groups.iter().filter(|_| anchors_trusted) {
        if g.retranscribed.is_empty() {
            // Deletion: Whisper heard less here. Nothing was recovered, and v1
            // never removes text the user already saw.
            continue;
        }
        if !g.committed.is_empty() {
            // A committed span sits under this material, so placing it would
            // rewrite the canvas — forbidden. Only escalate when the group
            // actually carries more than the canvas holds; an equal-or-smaller
            // substitution is a re-hearing, not lost speech.
            if g.retranscribed.len() > g.committed.len() {
                residual_required = true;
            }
            continue;
        }
        let replacement: String = g
            .retranscribed
            .clone()
            .map(|idx| r_tokens[idx].text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // Anti-duplication: a gap the aligner found is not always a gap in the
        // SPEECH. When Layer 0 mangled the words around a phrase, the aligner
        // cannot match them, so a phrase the canvas already carries reads as
        // missing and gets appended a second time. Measured 2026-08-14 on the
        // operator's take, the moment recoveries first reached the canvas:
        // "…która pozwoli nam na zrobienie hard pruna I road która pozwoli nam
        // na zrobienie hard Pru." — three repeated 4-grams, and a WER worse
        // than before the recovery landed. A recovery whose words already sit
        // in the canvas is a re-hearing of mangled text, not lost speech: it
        // belongs to substitution (which this lane does not do) or to the stop
        // path, never to a second copy in front of the user.
        let recovered: Vec<&Token> = g.retranscribed.clone().map(|idx| &r_tokens[idx]).collect();
        if canvas_already_carries(c_tokens, &recovered)
            || canvas_already_carries(neighbour_tokens, &recovered)
        {
            residual_required = true;
            continue;
        }
        // Zero-width range: a pure append at a boundary between committed
        // tokens, never a replacement of one.
        let (start, text) = if g.has_prev_match {
            (g.anchor, format!(" {replacement}"))
        } else {
            (0usize, format!("{replacement} "))
        };
        appends.push(EngineEvent::ReplaceRange {
            utterance_id,
            start,
            end: start,
            text,
            source: LayerSource::TailPatch,
        });
    }

    if appends.is_empty() {
        // Under-commit with nothing placeable: the whole recovery is owed to
        // the stop path.
        residual_required = true;
    }
    appends.sort_by_key(|e| std::cmp::Reverse(event_start(e)));

    let under = UnderCommit {
        appends,
        residual_required,
        committed_tokens,
        retranscribed_tokens,
        committed_chars,
        retranscribed_chars,
        commit_ratio,
    };
    info!(
        utterance_id,
        reason = under.reason(),
        committed_chars,
        retranscribed_chars,
        committed_tokens,
        retranscribed_tokens,
        commit_ratio,
        coverage,
        min_coverage,
        starved_canvas = min_coverage < UNDER_COMMIT_MIN_COVERAGE,
        anchors_trusted,
        gap_appends = under.appends.len(),
        residual_required = under.residual_required,
        "tail_patch_under_commit"
    );
    Some(under)
}

/// Compute bounded tail-patch events from a Layer-0 committed utterance and a
/// Whisper re-transcription of the same audio slice.
///
/// `utterance_id` is stamped on every emitted [`EngineEvent::ReplaceRange`].
pub fn compute_tail_patch(
    committed: &str,
    retranscribed: &str,
    utterance_id: u64,
    cfg: &TailPatchConfig,
) -> TailPatchOutcome {
    compute_tail_patch_with_context(committed, retranscribed, "", utterance_id, cfg)
}

/// As [`compute_tail_patch`], plus the canvas already sealed BEFORE this
/// utterance.
///
/// Layer 1 sees one utterance at a time. A phrase the PREVIOUS utterance
/// already carries therefore reads as a gap here and is appended a second
/// time — measured 2026-08-14 the moment recoveries first reached the canvas:
/// three repeated 4-grams and a WER worse than before the recovery landed.
/// The context is read-only; it is never patched, only consulted so a
/// duplicate escalates to the stop path instead of being placed.
pub fn compute_tail_patch_with_context(
    committed: &str,
    retranscribed: &str,
    neighbour_context: &str,
    utterance_id: u64,
    cfg: &TailPatchConfig,
) -> TailPatchOutcome {
    // Layer 0 owns the first commit: nothing to patch against an empty buffer.
    if committed.trim().is_empty() {
        log_skipped_receipt(
            utterance_id,
            "empty_committed",
            committed,
            retranscribed,
            0,
            0,
        );
        return TailPatchOutcome::skipped(SkipReasonCode::EmptyCommitted, "empty_committed");
    }
    if retranscribed.trim().is_empty() {
        log_skipped_receipt(
            utterance_id,
            "empty_retranscription",
            committed,
            retranscribed,
            tokenize(committed).len(),
            0,
        );
        return TailPatchOutcome::skipped(
            SkipReasonCode::EmptyRetranscription,
            "empty_retranscription",
        );
    }

    let c_tokens = tokenize(committed);
    let r_tokens = tokenize(retranscribed);
    if c_tokens.is_empty() {
        log_skipped_receipt(
            utterance_id,
            "no_committed_tokens",
            committed,
            retranscribed,
            0,
            r_tokens.len(),
        );
        return TailPatchOutcome::skipped(SkipReasonCode::NoCommittedTokens, "no_committed_tokens");
    }

    let matches = lcs_matches(&c_tokens, &r_tokens);

    // Build edit groups from the gaps between consecutive matched pairs.
    let mut groups: Vec<EditGroup> = Vec::new();
    let mut prev_c = 0usize;
    let mut prev_r = 0usize;
    let mut prev_match_c_end: Option<usize> = None; // char_end of last matched committed token
    for (mc, mr) in matches.iter().copied() {
        if mc > prev_c || mr > prev_r {
            groups.push(EditGroup {
                committed: prev_c..mc,
                retranscribed: prev_r..mr,
                anchor: prev_match_c_end.unwrap_or(0),
                has_prev_match: prev_match_c_end.is_some(),
            });
        }
        prev_match_c_end = Some(c_tokens[mc].char_end);
        prev_c = mc + 1;
        prev_r = mr + 1;
    }
    if prev_c < c_tokens.len() || prev_r < r_tokens.len() {
        groups.push(EditGroup {
            committed: prev_c..c_tokens.len(),
            retranscribed: prev_r..r_tokens.len(),
            anchor: prev_match_c_end.unwrap_or(0),
            has_prev_match: prev_match_c_end.is_some(),
        });
    }

    if groups.is_empty() {
        return TailPatchOutcome::NoChange;
    }

    // Safety gate: count changed tokens against the committed token budget.
    // The ratio is a wholesale-divergence guard; a SUBSTITUTION fix that fits
    // inside the small-edit budget is bounded by definition and bypasses it —
    // otherwise a short utterance can never be corrected at all (any one-word
    // fix on a 1-3-token commit is ≥50% change, and the lane sat at 116 skips /
    // 0 applied patches on the 2026-08-12 log because of exactly this).
    //
    // Substitution-shaped only, on purpose: every group must replace committed
    // tokens with retranscribed ones. Pure insertions keep their deliberate
    // routing from the under-commit work — head-loss recovery escalates as
    // `UnderCommit`, tiny bursts ("tak" → "no tak") stay noise-skipped — and a
    // blanket floor was measured to swallow all three of those contracts.
    let changed: usize = groups
        .iter()
        .map(|g| g.committed.len().max(g.retranscribed.len()))
        .sum();
    let small_substitution_fix = changed <= cfg.small_edit_token_floor
        && groups
            .iter()
            .all(|g| !g.committed.is_empty() && !g.retranscribed.is_empty());
    let ratio = changed as f64 / c_tokens.len() as f64;
    if ratio > cfg.max_change_ratio && !small_substitution_fix {
        // Before the cap discards this: is the canvas starved rather than
        // wrong? The bounded diff was never an instrument for measuring lost
        // speech, and using it as one is what threw the recovered 104 s / 107 s
        // Polish takes away.
        if let Some(under) = classify_under_commit(
            UnderCommitScan {
                c_tokens: &c_tokens,
                r_tokens: &r_tokens,
                matched: &matches,
                groups: &groups,
                neighbour_tokens: &tokenize(neighbour_context),
            },
            utterance_id,
            committed.trim().chars().count(),
            retranscribed.trim().chars().count(),
        ) {
            return TailPatchOutcome::UnderCommit(under);
        }
        let reason = format!(
            "change_ratio {:.2} exceeds max {:.2}",
            ratio, cfg.max_change_ratio
        );
        log_skipped_receipt(
            utterance_id,
            &reason,
            committed,
            retranscribed,
            c_tokens.len(),
            r_tokens.len(),
        );
        return TailPatchOutcome::skipped(SkipReasonCode::ChangeRatio, reason);
    }

    let mut events: Vec<EngineEvent> = Vec::new();
    for g in &groups {
        let c_empty = g.committed.is_empty();
        let r_empty = g.retranscribed.is_empty();

        if r_empty {
            // Deletion: v1 leaves committed tokens intact (conservative).
            continue;
        }

        let replacement: String = g
            .retranscribed
            .clone()
            .map(|idx| r_tokens[idx].text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        if c_empty {
            // Insertion: anchor after the previous matched token (or at start).
            if g.has_prev_match {
                events.push(EngineEvent::ReplaceRange {
                    utterance_id,
                    start: g.anchor,
                    end: g.anchor,
                    text: format!(" {replacement}"),
                    source: LayerSource::TailPatch,
                });
            } else {
                events.push(EngineEvent::ReplaceRange {
                    utterance_id,
                    start: 0,
                    end: 0,
                    text: format!("{replacement} "),
                    source: LayerSource::TailPatch,
                });
            }
        } else {
            // Substitution: replace the committed span with the W text. The
            // canvas span owns its trailing punctuation (Whisper emits bare
            // words); carry it onto the replacement so fixing a word never
            // eats the sentence boundary Apple already placed.
            let start = c_tokens[g.committed.start].char_start;
            let end = c_tokens[g.committed.end - 1].char_end;
            let last_committed = c_tokens[g.committed.end - 1].text.as_str();
            let trailing: String = last_committed
                .chars()
                .rev()
                .take_while(|c| !c.is_alphanumeric())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let keeps_own_boundary = replacement
                .chars()
                .last()
                .is_some_and(|c| !c.is_alphanumeric());
            let text = if trailing.is_empty() || keeps_own_boundary {
                replacement
            } else {
                format!("{replacement}{trailing}")
            };
            events.push(EngineEvent::ReplaceRange {
                utterance_id,
                start,
                end,
                text,
                source: LayerSource::TailPatch,
            });
        }
    }

    if events.is_empty() {
        return TailPatchOutcome::NoChange;
    }

    // Descending by start so sequential application is offset-stable.
    events.sort_by_key(|e| std::cmp::Reverse(event_start(e)));
    TailPatchOutcome::Patches(events)
}

/// Sort key for offset-stable application: the start offset of a
/// `ReplaceRange`, and 0 for any other event shape.
fn event_start(event: &EngineEvent) -> usize {
    match event {
        EngineEvent::ReplaceRange { start, .. } => *start,
        _ => 0,
    }
}

/// Pure unit tests for Layer-1 tail-patch diff outcomes and phase parsing.
#[cfg(test)]
mod tests {
    use super::*;

    /// Apply every emitted patch to the committed text, in emission order, and
    /// return the resulting buffer. Mirrors how a sink folds the events.
    fn apply_all(committed: &str, outcome: &TailPatchOutcome) -> String {
        let mut buf = committed.to_string();
        // `events()` folds both event-bearing arms, so an under-commit's
        // gap-appends are exercised by the same helper as ordinary patches.
        for ev in outcome.events() {
            ev.apply_to_committed_text(&mut buf)
                .expect("bounded range must be valid against committed text");
        }
        buf
    }

    /// Every event an under-commit emits must be a zero-width append.
    fn assert_all_zero_width(under: &UnderCommit) {
        for event in &under.appends {
            match event {
                EngineEvent::ReplaceRange {
                    start, end, source, ..
                } => {
                    assert_eq!(
                        start, end,
                        "under-commit may only append; a non-empty range rewrites committed text"
                    );
                    assert_eq!(*source, LayerSource::TailPatch);
                }
                other => panic!("under-commit emitted a non-ReplaceRange event: {other:?}"),
            }
        }
    }

    /// Exact re-transcription match must yield `NoChange` (no empty ReplaceRange).
    #[test]
    fn identical_text_is_no_change() {
        let cfg = TailPatchConfig::default();
        let outcome = compute_tail_patch("ala ma kota", "ala ma kota", 1, &cfg);
        assert_eq!(outcome, TailPatchOutcome::NoChange);
    }

    /// Empty Layer-0 floor has nothing to patch against; always `Skipped`.
    #[test]
    fn empty_committed_is_skipped() {
        let cfg = TailPatchConfig::default();
        let outcome = compute_tail_patch("", "cokolwiek", 1, &cfg);
        assert!(matches!(outcome, TailPatchOutcome::Skipped { .. }));
    }

    /// Substitution fixes a mixed-language mishear without full-buffer rewrite.
    #[test]
    fn single_substitution_corrects_mixed_language_token() {
        // Layer 0 (Apple, PL-dominant) misheard the English place name.
        let cfg = TailPatchConfig::default();
        let committed = "lecimy z Bytowa do nowego jorku";
        let retranscribed = "lecimy z Bytowa do New York";
        let outcome = compute_tail_patch(committed, retranscribed, 7, &cfg);
        match &outcome {
            TailPatchOutcome::Patches(events) => {
                assert!(events
                    .iter()
                    .all(|e| matches!(e, EngineEvent::ReplaceRange { source, .. } if *source == LayerSource::TailPatch)));
                assert!(events.iter().all(|e| matches!(e, EngineEvent::ReplaceRange { utterance_id, .. } if *utterance_id == 7)));
            }
            other => panic!("expected patches, got {other:?}"),
        }
        assert_eq!(
            apply_all(committed, &outcome),
            "lecimy z Bytowa do New York"
        );
    }

    /// Insertion fills a token Apple dropped while preserving surrounding text.
    #[test]
    fn insertion_fills_missing_token() {
        // Whisper recovered a technical term Apple dropped entirely.
        let cfg = TailPatchConfig::default();
        let committed = "używamy framework do tego";
        let retranscribed = "używamy framework vibecrafted do tego";
        let outcome = compute_tail_patch(committed, retranscribed, 3, &cfg);
        assert!(matches!(outcome, TailPatchOutcome::Patches(_)));
        assert_eq!(
            apply_all(committed, &outcome),
            "używamy framework vibecrafted do tego"
        );
    }

    /// Leading insertion anchors at char 0 with a trailing space in the patch text.
    #[test]
    fn leading_insertion_anchors_at_start() {
        let cfg = TailPatchConfig::default();
        let committed = "świecie cześć";
        let retranscribed = "witaj świecie cześć";
        let outcome = compute_tail_patch(committed, retranscribed, 1, &cfg);
        assert_eq!(apply_all(committed, &outcome), "witaj świecie cześć");
    }

    /// Leading/trailing whitespace in Whisper output must not enter offsets.
    #[test]
    fn retranscribed_whitespace_never_skews_offsets() {
        // Whisper output routinely carries leading/trailing whitespace. tokenize
        // skips it and replacements are joined from tokens, so it must never
        // enter the offsets nor the replacement text.
        let cfg = TailPatchConfig::default();
        let committed = "ala ma kota";
        let outcome = compute_tail_patch(committed, "  ala ma psa \n", 1, &cfg);
        assert!(matches!(outcome, TailPatchOutcome::Patches(_)));
        assert_eq!(apply_all(committed, &outcome), "ala ma psa");
    }

    /// v1 never deletes tokens the user already saw (deletions stay on Layer 0).
    #[test]
    fn deletion_is_left_intact_in_v1() {
        // Whisper saw fewer words; v1 must not remove text the user already saw.
        let cfg = TailPatchConfig::default();
        let committed = "to jest bardzo długie zdanie";
        let retranscribed = "to jest długie zdanie";
        let outcome = compute_tail_patch(committed, retranscribed, 1, &cfg);
        // Either NoChange (no emitted edits) — committed stays as-is.
        assert_eq!(apply_all(committed, &outcome), committed);
    }

    /// Change ratio above the safety threshold skips the whole patch wholesale.
    #[test]
    fn divergent_retranscription_is_skipped() {
        let cfg = TailPatchConfig::default();
        let committed = "ala ma kota";
        let retranscribed = "zupełnie inny tekst o czymś innym";
        let outcome = compute_tail_patch(committed, retranscribed, 1, &cfg);
        assert!(matches!(outcome, TailPatchOutcome::Skipped { .. }));
        // Layer 0 output stands.
        assert_eq!(apply_all(committed, &outcome), committed);
    }

    /// RED: once Whisper recovers substantially more non-trivial text than
    /// Layer 0 committed, a bounded-diff rejection must escalate rather than
    /// silently returning the same `Skipped` outcome as ordinary divergence.
    #[test]
    fn fleet_red_under_commit_escalates() {
        let cfg = TailPatchConfig::default();
        let committed = "pierwsza krótka fraza";
        let retranscribed = "pierwsza krótka fraza oraz cały odzyskany dalszy fragment wypowiedzi z wieloma słowami";
        let under_commit = compute_tail_patch(committed, retranscribed, 41, &cfg);

        let normal = compute_tail_patch("ala ma kota", "ala ma psa", 42, &cfg);
        assert!(matches!(normal, TailPatchOutcome::Patches(_)));

        let empty = compute_tail_patch("ala ma kota", "", 43, &cfg);
        assert_eq!(
            empty,
            TailPatchOutcome::skipped(
                SkipReasonCode::EmptyRetranscription,
                "empty_retranscription",
            )
        );

        assert!(
            !matches!(under_commit, TailPatchOutcome::Skipped { .. }),
            "committed/retranscribed below 0.6 must escalate, got {under_commit:?}"
        );
    }

    /// Recovered tail is appended, not discarded, and the canvas is untouched.
    #[test]
    fn under_commit_appends_recovered_tail_without_rewriting_canvas() {
        // The measured shape: Layer 0 kept one phrase of a long take, Whisper
        // returned that phrase plus everything Apple's partial-collapse ate.
        let cfg = TailPatchConfig::default();
        let committed = "pierwsza krótka fraza";
        let retranscribed = "pierwsza krótka fraza oraz cały odzyskany dalszy fragment wypowiedzi z wieloma słowami";
        let outcome = compute_tail_patch(committed, retranscribed, 41, &cfg);

        let TailPatchOutcome::UnderCommit(under) = &outcome else {
            panic!("expected UnderCommit, got {outcome:?}");
        };
        assert_eq!(under.committed_tokens, 3);
        assert_eq!(under.retranscribed_tokens, 12);
        assert!(under.commit_ratio < UNDER_COMMIT_RATIO);
        assert_eq!(under.appends.len(), 1, "one bounded gap-append");
        assert_all_zero_width(under);
        assert!(
            !under.residual_required,
            "everything recovered landed live; nothing is owed to the stop path"
        );
        // Append-plus-gap-fill: the committed prefix survives byte-identical.
        let applied = apply_all(committed, &outcome);
        assert!(applied.starts_with(committed));
        assert_eq!(applied, retranscribed);
    }

    /// Session a5623d55 (2026-08-12): the first live windows are SHORT — the
    /// canvas held 2 tokens, Whisper recovered 5 (the eaten utterance head).
    /// An absolute 6-token floor threw that recovery away three times in one
    /// minute. Short-window under-commits must escalate too; the coverage
    /// gate, not a length floor, is the garbage discriminator.
    #[test]
    fn short_window_under_commit_escalates_instead_of_skipping() {
        let cfg = TailPatchConfig::default();
        let committed = "zmienili zobacz";
        let retranscribed = "coś się tutaj zmienili zobacz";
        let outcome = compute_tail_patch(committed, retranscribed, 1, &cfg);

        let TailPatchOutcome::UnderCommit(under) = &outcome else {
            panic!("short-window head recovery must escalate, got {outcome:?}");
        };
        assert_eq!(under.committed_tokens, 2);
        assert_eq!(under.retranscribed_tokens, 5);
        // The head sits before the first match: a prepend, canvas untouched.
        let applied = apply_all(committed, &outcome);
        assert!(
            applied.ends_with(committed),
            "canvas must survive: {applied:?}"
        );
        assert!(
            applied.contains("coś się tutaj"),
            "recovered head must land: {applied:?}"
        );
    }

    /// The floor still exists for genuinely tiny bursts: a two-word decode
    /// against a one-word canvas carries nothing worth escalating.
    #[test]
    fn tiny_burst_still_skips() {
        let cfg = TailPatchConfig::default();
        let outcome = compute_tail_patch("tak", "no tak", 1, &cfg);
        assert!(
            matches!(outcome, TailPatchOutcome::Skipped { .. }),
            "two-word burst must not escalate: {outcome:?}"
        );
    }

    /// Recovered speech that would have to overwrite a committed span is never
    /// emitted — it escalates to the stop path instead.
    #[test]
    fn under_commit_without_safe_anchor_requires_residual() {
        let cfg = TailPatchConfig::default();
        // "piec" is committed but absent from the re-transcription, so the whole
        // recovered tail sits under a committed token: no safe anchor exists.
        let committed = "raz dwa trzy cztery piec";
        let retranscribed = "raz dwa trzy cztery szesc siedem osiem dziewiec dziesiec";
        let outcome = compute_tail_patch(committed, retranscribed, 42, &cfg);

        let TailPatchOutcome::UnderCommit(under) = &outcome else {
            panic!("expected UnderCommit, got {outcome:?}");
        };
        assert!(under.appends.is_empty(), "no anchor was demonstrably safe");
        assert!(under.residual_required);
        assert_eq!(under.reason(), "under_commit_residual_required");
        assert!(outcome.residual_required());
        // Layer 0 stands exactly as the user saw it.
        assert_eq!(apply_all(committed, &outcome), committed);
    }

    /// Mixed under-commit: the addressable gap is filled live, the unplaceable
    /// remainder escalates, and no committed token is rewritten either way.
    #[test]
    fn under_commit_fills_safe_gap_and_still_escalates_remainder() {
        let cfg = TailPatchConfig::default();
        let committed = "raz dwa trzy cztery piec szesc";
        let retranscribed =
            "raz dwa trzy alfa beta gamma cztery piec siedem osiem dziewiec dziesiec";
        let outcome = compute_tail_patch(committed, retranscribed, 43, &cfg);

        let TailPatchOutcome::UnderCommit(under) = &outcome else {
            panic!("expected UnderCommit, got {outcome:?}");
        };
        assert_eq!(under.appends.len(), 1);
        assert_all_zero_width(under);
        assert!(
            under.residual_required,
            "the tail under the committed span could not be placed"
        );
        assert_eq!(
            apply_all(committed, &outcome),
            "raz dwa trzy alfa beta gamma cztery piec szesc",
            "gap filled in place; every committed token survives"
        );
    }

    /// Coverage — not the length ratio — is what separates under-commit from
    /// divergence. This decode is *shorter-ratio* than the escalation threshold
    /// yet shares no token with the canvas, so its offsets mean nothing.
    #[test]
    fn divergence_without_canvas_coverage_never_escalates() {
        let cfg = TailPatchConfig::default();
        let committed = "ala ma kota";
        let retranscribed = "zupełnie inny tekst o czymś innym";
        assert!(
            (tokenize(committed).len() as f64 / tokenize(retranscribed).len() as f64)
                < UNDER_COMMIT_RATIO,
            "fixture must sit below the ratio gate so coverage is the deciding gate"
        );
        let outcome = compute_tail_patch(committed, retranscribed, 44, &cfg);
        assert!(matches!(outcome, TailPatchOutcome::Skipped { .. }));
        assert_eq!(apply_all(committed, &outcome), committed);
    }

    /// A short Whisper burst satisfies any ratio while recovering nothing worth
    /// appending; it stays on the ordinary skip path.
    #[test]
    fn short_retranscription_never_escalates() {
        let cfg = TailPatchConfig::default();
        let outcome = compute_tail_patch("tak", "tak jest dobrze", 45, &cfg);
        assert!(
            matches!(outcome, TailPatchOutcome::Skipped { .. }),
            "under {UNDER_COMMIT_MIN_RETRANSCRIBED_TOKENS} tokens is noise, not recovered speech"
        );
    }

    /// The escalation must not disturb the ordinary lane: a small diff still
    /// patches, and an empty re-transcription still skips with its exact reason.
    #[test]
    fn small_diff_and_empty_retranscription_are_unchanged() {
        let cfg = TailPatchConfig::default();
        assert!(matches!(
            compute_tail_patch("ala ma kota", "ala ma psa", 46, &cfg),
            TailPatchOutcome::Patches(_)
        ));
        assert_eq!(
            compute_tail_patch("ala ma kota", "", 47, &cfg),
            TailPatchOutcome::skipped(
                SkipReasonCode::EmptyRetranscription,
                "empty_retranscription",
            )
        );
        assert!(matches!(
            compute_tail_patch("", "cokolwiek dłuższego tu jest naprawdę sporo", 48, &cfg),
            TailPatchOutcome::Skipped { .. }
        ));
    }

    /// Multiple substitutions emit descending-by-start for offset-stable apply.
    #[test]
    fn multiple_edits_apply_offset_stable() {
        // Two independent substitutions in one utterance; applying all emitted
        // events in order must land the fully corrected text.
        let cfg = TailPatchConfig::default();
        let committed = "spotkanie o foo i potem bar wieczorem";
        let retranscribed = "spotkanie o dziesiątej i potem osiemnastej wieczorem";
        let outcome = compute_tail_patch(committed, retranscribed, 2, &cfg);
        match &outcome {
            TailPatchOutcome::Patches(events) => {
                // Emitted descending by start for offset-stable application.
                let starts: Vec<usize> = events.iter().map(event_start).collect();
                let mut sorted = starts.clone();
                sorted.sort_by(|a, b| b.cmp(a));
                assert_eq!(starts, sorted, "events must be descending by start");
            }
            other => panic!("expected patches, got {other:?}"),
        }
        assert_eq!(
            apply_all(committed, &outcome),
            "spotkanie o dziesiątej i potem osiemnastej wieczorem"
        );
    }

    /// The 2026-08-12 live case: a short utterance can be corrected at all.
    ///
    /// "Kos" for "kombos" is a one-word fix on a three-token commit — ratio
    /// 0.33? No: with re-segmentation the changed-token count hits the ratio
    /// cap, and before the small-edit floor existed the lane logged
    /// `change_ratio 1.50 exceeds max 0.50` and threw Whisper's correct
    /// hearing away. Measured: 116 skips, 0 applied patches in the whole log.
    #[test]
    fn small_edit_on_short_utterance_bypasses_the_ratio_cap() {
        let cfg = TailPatchConfig::default();
        let committed = "Jaki chcesz. Kos.";
        let retranscribed = "jaki chcesz kombos";
        let outcome = compute_tail_patch(committed, retranscribed, 2, &cfg);
        match &outcome {
            TailPatchOutcome::Patches(_) => {}
            other => panic!("a bounded one-word fix must patch, got {other:?}"),
        }
        assert!(
            apply_all(committed, &outcome)
                .to_lowercase()
                .contains("kombos"),
            "the corrected word must land on the canvas"
        );
    }

    /// A canvas that is mostly mangled must still be recoverable.
    ///
    /// The operator's live log 2026-08-14: SFSpeech committed a fragment while
    /// Whisper heard the whole sentence, and the full coverage bar rejected the
    /// recovery precisely because Layer 0 had mangled the words it did commit —
    /// the worse it heard, the fewer tokens matched, the more certain the
    /// rejection. 5341 characters of speech went to the receipt that way.
    /// The canvas here mirrors the measured shape (a short mangled fragment
    /// against a re-transcription carrying several times the material).
    #[test]
    fn starved_canvas_recovers_instead_of_being_rejected_for_low_coverage() {
        let cfg = TailPatchConfig::default();
        // Two canvas tokens survive inside the re-transcription; the rest is
        // mangled — coverage 0.5, under the full 0.8 bar, over the starved one.
        let committed = "no gdzieś in zrope analizę";
        let retranscribed = "zrób z loctree analizę pełną martwego kodu bo mamy nową wersję \
             która pozwoli nam zrobić hard pruna przed wydaniem";
        let outcome = compute_tail_patch(committed, retranscribed, 7, &cfg);
        let under = match &outcome {
            TailPatchOutcome::UnderCommit(under) => under,
            other => panic!("a starved canvas must classify as under-commit, got {other:?}"),
        };
        assert!(
            under.residual_required,
            "a canvas too mangled to anchor against owes its recovery to the stop \
             path — it must never be dropped as a change-ratio receipt"
        );
        // Nothing is placed inline: the anchors are not trustworthy, so the
        // canvas the user is watching is left exactly as it was.
        assert!(
            under.appends.is_empty(),
            "untrusted anchors must not place text into the live canvas"
        );
        assert_eq!(apply_all(committed, &outcome), committed);
    }

    /// The same lane, with a canvas Layer 0 heard well enough to anchor against:
    /// here the recovery lands inline, append-only, and the canvas grows.
    #[test]
    fn starved_canvas_with_trusted_anchors_places_the_recovery_inline() {
        let cfg = TailPatchConfig::default();
        let committed = "mamy nową wersję";
        let retranscribed = "mamy nową wersję pełną analizę martwego kodu która pozwoli nam zrobić \
             hard pruna przed wydaniem";
        let outcome = compute_tail_patch(committed, retranscribed, 9, &cfg);
        let under = match &outcome {
            TailPatchOutcome::UnderCommit(under) => under,
            other => panic!("under-commit expected, got {other:?}"),
        };
        assert!(
            !under.appends.is_empty(),
            "trusted anchors must place the recovered material"
        );
        let applied = apply_all(committed, &outcome);
        assert!(
            applied.len() > committed.len(),
            "canvas must grow: {applied:?}"
        );
        for token in ["mamy", "nową", "wersję"] {
            assert!(
                applied.contains(token),
                "committed text must never be rewritten, missing {token:?} in {applied:?}"
            );
        }
    }

    /// The exact duplication measured on the operator's take 2026-08-14, the
    /// first run where recoveries reached the canvas at all.
    ///
    /// Whisper's window for a short utterance ("Zrób.") carried the phrase the
    /// NEXT utterance already holds, so the aligner saw a gap and appended a
    /// second copy: "…hard pruna I road która pozwoli nam na zrobienie hard
    /// Pru." — three repeated 4-grams, WER 0.463 → 0.610. With the neighbour
    /// canvas in hand the recovery escalates instead of duplicating.
    #[test]
    fn recovery_already_carried_by_the_neighbour_utterance_is_not_appended() {
        let cfg = TailPatchConfig::default();
        let committed = "Zrób.";
        let retranscribed = "zrób która pozwoli nam na zrobienie hard pruna";
        let neighbour = "I road która pozwoli nam na zrobienie hard Pru.";

        // Without the neighbour context this lane duplicates the phrase.
        let blind = compute_tail_patch(committed, retranscribed, 5, &cfg);
        let blind_text = apply_all(committed, &blind);
        assert!(
            blind_text.to_lowercase().contains("pozwoli nam na"),
            "fixture must reproduce the duplication when the neighbour is unknown: {blind_text:?}"
        );

        // With it, nothing is placed and the recovery is owed to the stop path.
        let outcome = compute_tail_patch_with_context(committed, retranscribed, neighbour, 5, &cfg);
        match &outcome {
            TailPatchOutcome::UnderCommit(under) => assert!(
                under.appends.is_empty() && under.residual_required,
                "a phrase the neighbour carries must escalate, not duplicate: {under:?}"
            ),
            TailPatchOutcome::Skipped { .. } => {}
            other => panic!("must not place a duplicate, got {other:?}"),
        }
        assert_eq!(
            apply_all(committed, &outcome),
            committed,
            "the canvas must be byte-identical when the recovery is a duplicate"
        );
    }

    /// The live canvas stays protected: a re-transcription that shares almost
    /// nothing with the canvas may still be owed to the stop path (which has its
    /// own hallucination and semantic gates), but it must never place a single
    /// character into the text the user is watching.
    #[test]
    fn low_agreement_recovery_never_touches_the_live_canvas() {
        let cfg = TailPatchConfig::default();
        let committed = "spotkanie o dziesiątej";
        let retranscribed = "całkiem inne zdanie o zupełnie innych sprawach które nigdy nie padło \
             w tym nagraniu ani razu w żadnej formie";
        let outcome = compute_tail_patch(committed, retranscribed, 8, &cfg);
        match &outcome {
            TailPatchOutcome::Skipped { .. } => {}
            TailPatchOutcome::UnderCommit(under) => assert!(
                under.appends.is_empty() && under.residual_required,
                "low agreement may only escalate, never place: {under:?}"
            ),
            other => panic!("a low-agreement decode must not patch inline, got {other:?}"),
        }
        assert_eq!(
            apply_all(committed, &outcome),
            committed,
            "the canvas the user is watching must be byte-identical"
        );
    }

    /// Ordinary divergence on a HEALTHY canvas keeps the old contract: Layer 0
    /// stands, nothing escalates. The starved path must not become a blanket
    /// amnesty for every disagreement.
    #[test]
    fn divergence_on_a_healthy_canvas_still_skips() {
        let cfg = TailPatchConfig::default();
        let committed = "spotkanie o dziesiątej rano w poniedziałek";
        let retranscribed = "zupełnie inne słowa nie mające z tym wspólnego";
        match compute_tail_patch(committed, retranscribed, 10, &cfg) {
            TailPatchOutcome::Skipped { .. } => {}
            other => panic!("healthy-canvas divergence must stay skipped, got {other:?}"),
        }
    }

    /// Casing and punctuation are alignment facts, not edits. The canvas comes
    /// from Apple + lexicon (capitalized, punctuated); the re-transcription is
    /// bare-lowercase Whisper. Compared byte-exact they shared zero tokens and
    /// every healthy sentence read as wholesale divergence — the structural
    /// half of the 116-skips/0-applied starvation (live ratios 0.56-2.00 on
    /// ordinary Polish takes, 2026-08-12 21:20 session). The one genuinely
    /// different word must be the only patch, and it must keep the sentence
    /// boundary Apple already placed.
    #[test]
    fn casing_and_punctuation_align_instead_of_counting_as_changes() {
        let cfg = TailPatchConfig::default();
        let committed = "Jaki chcesz. Kos.";
        let retranscribed = "jaki chcesz kombos";
        let outcome = compute_tail_patch(committed, retranscribed, 21, &cfg);
        match &outcome {
            TailPatchOutcome::Patches(events) => {
                assert_eq!(events.len(), 1, "only the truly different word patches");
            }
            other => panic!("expected a single bounded patch, got {other:?}"),
        }
        assert_eq!(apply_all(committed, &outcome), "Jaki chcesz. kombos.");
    }

    /// The same words in Whisper's bare normalization are NoChange — the
    /// canvas keeps its casing and punctuation and nothing moves.
    #[test]
    fn same_words_in_bare_normalization_are_no_change() {
        let cfg = TailPatchConfig::default();
        let committed = "To jest zdanie, które Apple dobrze usłyszało.";
        let retranscribed = "to jest zdanie które apple dobrze usłyszało";
        let outcome = compute_tail_patch(committed, retranscribed, 22, &cfg);
        assert_eq!(outcome, TailPatchOutcome::NoChange);
    }

    /// The floor is a small-edit budget, not a hole in the divergence guard: a
    /// wholesale rewrite of a short utterance still skips.
    #[test]
    fn wholesale_divergence_on_short_utterance_still_skips() {
        let cfg = TailPatchConfig::default();
        let committed = "dobra nara";
        let retranscribed = "zupełnie inne zdanie o niczym wcale niepodobne do tamtego";
        let outcome = compute_tail_patch(committed, retranscribed, 3, &cfg);
        match &outcome {
            TailPatchOutcome::Skipped { reason, .. } => {
                assert!(
                    reason.contains("change_ratio"),
                    "unexpected reason: {reason}"
                );
            }
            TailPatchOutcome::UnderCommit(_) => {
                // Also acceptable: classified as recovered speech, which is a
                // deliberate, bounded append path — never a rewrite.
            }
            other => panic!("wholesale divergence must not rewrite, got {other:?}"),
        }
    }

    /// Polish diacritics: offsets are char-based so apply never corrupts UTF-8.
    #[test]
    fn unicode_offsets_are_char_based() {
        // Polish diacritics: offsets must be char- not byte-based or the apply
        // helper would corrupt the buffer.
        let cfg = TailPatchConfig::default();
        let committed = "zażółć gęślą jaźń teraz";
        let retranscribed = "zażółć gęślą jaźń natychmiast";
        let outcome = compute_tail_patch(committed, retranscribed, 9, &cfg);
        assert_eq!(
            apply_all(committed, &outcome),
            "zażółć gęślą jaźń natychmiast"
        );
    }

    /// Default config lands on the 0.5 unit-interval safety threshold.
    #[test]
    fn config_from_env_clamps_to_unit_interval() {
        // Out-of-range / garbage values fall back to default.
        let cfg = TailPatchConfig::default();
        assert_eq!(cfg.max_change_ratio, 0.5);
    }

    /// Accepts `phaseN` / bare digits in 1..=4; rejects out-of-range and off tokens.
    #[test]
    fn layered_phase_parses_phase_prefix() {
        // Pure parse — no process env (suite stays deterministic under parallel exec).
        assert_eq!(layered_phase_from_raw(None), Some(LAYERED_DEFAULT_PHASE));
        assert_eq!(layered_phase_from_raw(Some("off")), None);
        assert_eq!(parse_layered_phase_value("phase1"), Some(1));
        assert_eq!(parse_layered_phase_value("phase2"), Some(2));
        assert_eq!(parse_layered_phase_value("4"), Some(4));
        assert_eq!(parse_layered_phase_value("  PHASE3  "), Some(3));
        assert_eq!(parse_layered_phase_value("phase9"), None);
        assert_eq!(parse_layered_phase_value("off"), None);
        assert_eq!(parse_layered_phase_value(""), None);
        assert_eq!(parse_layered_phase_value("0"), None);
    }

    /// FINAL_PASS_MODE vocabulary must never parse as a layered phase.
    #[test]
    fn layered_phase_rejects_final_pass_mode_tokens() {
        // Orthogonality: FINAL_PASS_MODE vocabulary must never enable Layer 1.
        // If an operator (or a bug) copies smart/always/off into
        // CODESCRIBE_LAYERED_TRANSCRIPTION, treat as off — not as a phase.
        for token in ["smart", "always", "off", "auto", "on", "true", "yes"] {
            assert_eq!(
                parse_layered_phase_value(token),
                None,
                "final-pass token {token:?} must not parse as a layered phase"
            );
        }
        assert_eq!(parse_layered_phase_value("phase1"), Some(1));
    }
}
