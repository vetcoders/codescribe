//! W13-3B — Silero utterance identity + conservative per-word fusion.
//!
//! The product-owned settings snapshot arms this mandatory lane by default;
//! [`SILERO_FUSION_ENV`] remains an optional power-user override. When armed:
//! Silero Supervisor edges supply boundary, time, and energy evidence on the
//! PCM sample clock; Apple cumulative finals are sliced onto those ranges by
//! time; Whisper and Apple then fuse conservatively (agreements + clear gap
//! fills). Unresolved alternatives are receipted, never confidence-arbitrated.
//! Text-bearing results return to the Apple session: `admit_ledger_label`
//! offers each observation to `AcousticLedger::admit`, closed occurrences pass
//! through `AcousticLedger::seal`, and ledger events reach the transcript
//! reducer. This module owns no text admission or seal authority.
//!
//! # One Silero per session
//!
//! [`SileroIngress`] is the session's **only** `SpeechSession`. Both consumers
//! of speech edges read it: Silero boundary-range bookkeeping and the
//! Apple engine lifecycle (`EpochGate` wake/sleep). Two independent VAD
//! sessions over the same PCM would mean two spectra and two sets of
//! boundaries, and "the same utterance" would then mean two different sample
//! ranges depending on which consumer was asked. [`SileroIngress::observe`] is
//! the single decision point that derives both from one observation.

use std::collections::VecDeque;

use crate::audio::chunker::{SpeechEvent, SpeechSession, VadBoundaryEvidence, VadBoundaryKind};
use crate::config::RuntimeSettingsSnapshot;
use crate::pipeline::contracts::{
    NonSpeechEvidence, SidebandEvidence, SidebandEvidenceKind, SidebandProvenance,
};
use crate::stt::tail_provider::{TailSampleRange, TimedTailSegment};

/// Stable name of the optional power-user override. Its value is resolved by
/// the canonical settings loader, never in this pipeline module.
pub use crate::config::settings::SILERO_FUSION_ENV;

/// Bounded-context A/B selector. Never crosses a long-silence cut.
pub const SILERO_FUSION_CONTEXT_ENV: &str = "CODESCRIBE_SILERO_FUSION_CONTEXT";

/// Silence longer than this (samples at the capture rate) is a hard context
/// fence — left-audio pad must not reach across it.
pub const LONG_SILENCE_FENCE_SECS: f32 = 0.55;

/// Default left-audio pad when [`FusionContextMode::LeftAudioPad`] is armed.
pub const DEFAULT_LEFT_PAD_SECS: f32 = 0.40;

/// Maximum sideband facts retained for later span attachment. Live consumers
/// receive every freshly emitted fact; only the retrospective lookup window is
/// bounded.
const MAX_RETAINED_SIDEBAND_EVIDENCE: usize = 512;

/// Whether the seal lane can bound existence for a product take, probed with
/// no session open: the sealed product setting and whether the shared Silero graph loads.
/// `seal_utterance_final` passes `may_qualify = silero_bound`, so without
/// both no occurrence can ever qualify — admission readiness must ask first
/// instead of letting a take record into a ledger that cannot seal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealLaneProbe {
    /// The immutable runtime settings generation resolved the lane as armed.
    pub armed: bool,
    /// The embedded Silero model actually loaded in this process.
    pub vad_available: bool,
}

/// Probe the seal lane. Cheap after the first call: the Silero session is a
/// process-wide `OnceLock`.
pub fn seal_lane_probe(snapshot: &RuntimeSettingsSnapshot) -> SealLaneProbe {
    let armed = snapshot.seal_lane_armed();
    let vad_available = SileroIngress::new(16_000, "admission-probe", 0).vad_available();
    SealLaneProbe {
        armed,
        vad_available,
    }
}

/// One Silero-bounded utterance on the session PCM clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SileroUtterance {
    pub id: u64,
    pub range: TailSampleRange,
    pub closed: bool,
}

/// Silero boundary-range bookkeeping. Pure data; the Supervisor machine in
/// [`SileroIngress`] is the only writer in production. This is not the
/// [`crate::pipeline::acoustic_ledger::AcousticLedger`] and owns no text,
/// occurrence admission, or seal authority.
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
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Newly measured content-free evidence, in PCM/sequence order.
    pub sideband: Vec<SidebandEvidence>,
}

/// Supervisor-mode Silero at the Apple PCM ingress. The session's only VAD.
pub struct SileroIngress {
    session: String,
    capture_epoch: u64,
    sample_rate: u32,
    vad: SpeechSession,
    ledger: UtteranceLedger,
    next_sideband_sequence: u64,
    last_speech_end: Option<u64>,
    sideband: VecDeque<SidebandEvidence>,
}

impl SileroIngress {
    pub fn new(sample_rate: u32, session: impl Into<String>, capture_epoch: u64) -> Self {
        Self {
            session: session.into(),
            capture_epoch,
            sample_rate,
            vad: SpeechSession::new_utterance(sample_rate),
            ledger: UtteranceLedger::new(),
            next_sideband_sequence: 0,
            last_speech_end: None,
            sideband: VecDeque::new(),
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
        let boundaries = self.vad.take_vad_boundaries();
        let closed_here = events
            .iter()
            .any(|event| matches!(event, SpeechEvent::UtteranceFinal));
        let open_range = self.vad.open_segment_raw_range();
        let mut out = self.observe(open_range, closed_here, samples_seen);
        out.sideband = self.observe_boundaries(&boundaries);
        out
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

    /// Convert the chunker's exact Silero boundaries into ordered pipeline
    /// evidence. This is intentionally separate from [`Self::observe`]: the
    /// fusion ledger retains its padded STT window semantics, while sideband
    /// evidence names the unpadded threshold crossings exactly.
    pub(crate) fn observe_boundaries(
        &mut self,
        boundaries: &[VadBoundaryEvidence],
    ) -> Vec<SidebandEvidence> {
        let mut emitted = Vec::new();
        for boundary in boundaries {
            match boundary.kind {
                VadBoundaryKind::SpeechStart => {
                    if let Some(pause_start) = self.last_speech_end.take()
                        && pause_start < boundary.sample
                    {
                        emitted.push(self.push_sideband(
                            pause_start,
                            boundary.sample,
                            SidebandEvidenceKind::Pause {
                                duration_samples: boundary.sample - pause_start,
                                non_speech: NonSpeechEvidence::UnknownNonSpeech,
                            },
                        ));
                    }
                    emitted.push(self.push_sideband(
                        boundary.sample,
                        boundary.sample,
                        SidebandEvidenceKind::SpeechStart {
                            speech_probability: boundary.speech_probability,
                        },
                    ));
                }
                VadBoundaryKind::SpeechEnd => {
                    emitted.push(self.push_sideband(
                        boundary.sample,
                        boundary.sample,
                        SidebandEvidenceKind::SpeechEnd {
                            speech_probability: boundary.speech_probability,
                        },
                    ));
                    self.last_speech_end = Some(boundary.sample);
                }
            }
        }
        self.sideband.extend(emitted.iter().cloned());
        while self.sideband.len() > MAX_RETAINED_SIDEBAND_EVIDENCE {
            self.sideband.pop_front();
        }
        emitted
    }

    fn push_sideband(
        &mut self,
        sample_start: u64,
        sample_end: u64,
        evidence: SidebandEvidenceKind,
    ) -> SidebandEvidence {
        self.next_sideband_sequence = self.next_sideband_sequence.saturating_add(1);
        SidebandEvidence {
            sequence: self.next_sideband_sequence,
            range: TailSampleRange {
                session: self.session.clone(),
                capture_epoch: self.capture_epoch,
                sample_start,
                sample_end: sample_end.max(sample_start),
            },
            sample_rate_hz: self.sample_rate,
            provenance: SidebandProvenance::SileroVad,
            evidence,
        }
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

    /// Sideband claims stop exactly where Silero's evidence stops: threshold
    /// edges plus an unknown non-speech pause between them.
    #[test]
    fn sideband_edges_and_pause_keep_exact_pcm_ranges_and_order() {
        let mut ingress = SileroIngress::new(16_000, "s", 4);
        let first = ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 8_000,
                speech_probability: 0.81,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 24_000,
                speech_probability: 0.12,
            },
        ]);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].sequence, 1);
        assert_eq!(first[0].range.sample_start, 8_000);
        assert_eq!(first[0].range.sample_end, 8_000);
        assert!(matches!(
            first[0].evidence,
            SidebandEvidenceKind::SpeechStart { .. }
        ));
        assert_eq!(first[1].sequence, 2);
        assert_eq!(first[1].range.sample_start, 24_000);
        assert_eq!(first[1].range.sample_end, 24_000);

        let resumed = ingress.observe_boundaries(&[VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechStart,
            sample: 40_000,
            speech_probability: 0.76,
        }]);
        assert_eq!(resumed.len(), 2, "pause then the resuming speech edge");
        assert_eq!(resumed[0].sequence, 3);
        assert_eq!(resumed[0].range.sample_start, 24_000);
        assert_eq!(resumed[0].range.sample_end, 40_000);
        assert!(matches!(
            resumed[0].evidence,
            SidebandEvidenceKind::Pause {
                duration_samples: 16_000,
                non_speech: NonSpeechEvidence::UnknownNonSpeech,
            }
        ));
        assert_eq!(resumed[1].sequence, 4);
        assert!(matches!(
            resumed[1].evidence,
            SidebandEvidenceKind::SpeechStart { .. }
        ));
    }

    /// Long hands-free takes cannot retain one sideband row per speech edge
    /// forever; sequence identity remains global while lookup memory is
    /// capped to the newest evidence.
    #[test]
    fn retained_sideband_evidence_is_bounded_without_reusing_sequence_ids() {
        let mut ingress = SileroIngress::new(16_000, "long", 1);
        for index in 0..(MAX_RETAINED_SIDEBAND_EVIDENCE + 20) {
            let emitted = ingress.observe_boundaries(&[VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: index as u64,
                speech_probability: 0.1,
            }]);
            assert_eq!(emitted.len(), 1);
        }

        assert_eq!(ingress.sideband.len(), MAX_RETAINED_SIDEBAND_EVIDENCE);
        assert_eq!(
            ingress.sideband.front().expect("retained first").sequence,
            21
        );
        assert_eq!(
            ingress.sideband.back().expect("retained last").sequence,
            (MAX_RETAINED_SIDEBAND_EVIDENCE + 20) as u64
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

    /// Regression proof: active boundary events (UtteranceFinal discriminant) and
    /// VAD sideband evidence survive payload/fusion cleanup with exact PCM ranges.
    #[test]
    fn regression_boundary_emission_and_vad_evidence_survive_payload_cleanup() {
        let mut ingress = SileroIngress::new(16_000, "regression_session", 1);
        let boundaries = vec![
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 16_000,
                speech_probability: 0.88,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 32_000,
                speech_probability: 0.15,
            },
        ];
        let sideband = ingress.observe_boundaries(&boundaries);
        assert_eq!(sideband.len(), 2);
        assert_eq!(sideband[0].range.sample_start, 16_000);
        assert_eq!(sideband[1].range.sample_start, 32_000);
        assert!(matches!(
            sideband[0].evidence,
            SidebandEvidenceKind::SpeechStart { .. }
        ));

        // Observation closed segment check
        let event = SpeechEvent::UtteranceFinal;
        assert!(
            matches!(event, SpeechEvent::UtteranceFinal),
            "unit boundary discriminant must match SpeechEvent::UtteranceFinal"
        );
        let obs = ingress.observe(Some((16_000, 32_000)), true, 32_000);
        assert_eq!(obs.closed, vec![1]);
        assert_eq!(ingress.ledger().utterances().len(), 1);
        assert_eq!(ingress.ledger().utterances()[0].range.sample_start, 16_000);
        assert_eq!(ingress.ledger().utterances()[0].range.sample_end, 32_000);
    }
}
