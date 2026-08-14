//! W13-4 — sealed-span idempotence + in-span loop fence.
//!
//! Ports [`crate::asr_session::SessionIngest`] sealed-utterance rules onto the
//! live seal ledger. Identity is the canonical PCM range
//! (`session`, `capture_epoch`, `sample_start`, `sample_end`) plus an optional
//! provider `request_id`. Text is never a suppression key.
//!
//! Auto-removal is allowed only on non-content evidence (Amendment 3 / D2):
//! replayed request/range identity, non-progressing timestamps, or a decode
//! failure. Anything else is kept; a content-similar offer against a *new*
//! identity emits a WARN receipt and still lands on the canvas.
//!
//! Lane flag [`SPAN_IDEMPOTENCE_ENV`] is **default OFF**.

use std::collections::BTreeSet;

use crate::stt::tail_provider::{TailRequestIdentity, TailSampleRange};

/// Lane flag for sealed-span replay refusal. Unset / `0` / `false` / `off` /
/// `no` keep the pre-W13-4 seal path bit-identical.
pub const SPAN_IDEMPOTENCE_ENV: &str = "CODESCRIBE_SPAN_IDEMPOTENCE";

/// Whether the W13-4 idempotence lane is armed. Default OFF.
pub fn lane_enabled() -> bool {
    let raw = std::env::var(SPAN_IDEMPOTENCE_ENV).ok();
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

/// Non-content evidence that may auto-remove a delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonContentEvidence {
    /// Exact `(session, epoch, start, end)` already sealed or accepted.
    ReplayedRangeIdentity,
    /// Same `request_id` already consumed (provider re-submit).
    ReplayedRequestIdentity,
    /// Word/span clock did not advance on a re-offer of the same request.
    NonProgressingTimestamps,
    /// Provider reported a failed decode for this identity.
    DecodeFailure,
}

impl NonContentEvidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReplayedRangeIdentity => "replayed_range_identity",
            Self::ReplayedRequestIdentity => "replayed_request_identity",
            Self::NonProgressingTimestamps => "non_progressing_timestamps",
            Self::DecodeFailure => "decode_failure",
        }
    }
}

/// What the ledger decided about one offered span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanOfferVerdict {
    /// New identity; canvas grows.
    Accepted,
    /// Same sealed/accepted identity — SessionIngest `RejectedSealedUtterance`
    /// / `DuplicateIdempotent` ported onto range identity.
    RejectedSealedReplay,
    /// In-span loop fenced on non-content evidence (auto-removed).
    FencedLoop { evidence: NonContentEvidence },
    /// Content looks like a duplicate but the identity is new — KEEP.
    WarnPreserved,
}

impl SpanOfferVerdict {
    pub fn lands_on_canvas(&self) -> bool {
        matches!(self, Self::Accepted | Self::WarnPreserved)
    }

    pub fn as_token(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::RejectedSealedReplay => "rejected_sealed_replay",
            Self::FencedLoop { .. } => "fenced_loop",
            Self::WarnPreserved => "content_similar_preserved",
        }
    }
}

/// Content-free receipt. Never carries transcript text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanIdempotenceReceipt {
    pub code: &'static str,
    pub warn: bool,
    pub request_id: u64,
    pub range: TailSampleRange,
}

/// One offered delivery. `text` is for canvas assembly only — never a key.
#[derive(Debug, Clone)]
pub struct SpanOffer {
    pub identity: TailRequestIdentity,
    pub text: String,
    /// Caller-measured: did word/span timestamps advance vs the previous
    /// offer of this `request_id`? Unused on a first offer.
    pub timestamps_progressed: bool,
    pub decode_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcceptedSpan {
    identity: TailRequestIdentity,
    text: String,
}

/// Sealed-span ledger. Holds no audio and reads no wall clock.
#[derive(Debug, Clone, Default)]
pub struct SpanIdempotenceLedger {
    accepted: Vec<AcceptedSpan>,
    sealed_ranges: BTreeSet<RangeKey>,
    receipts: Vec<SpanIdempotenceReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RangeKey {
    session: String,
    capture_epoch: u64,
    sample_start: u64,
    sample_end: u64,
}

impl RangeKey {
    fn from_range(range: &TailSampleRange) -> Self {
        Self {
            session: range.session.clone(),
            capture_epoch: range.capture_epoch,
            sample_start: range.sample_start,
            sample_end: range.sample_end,
        }
    }
}

impl SpanIdempotenceLedger {
    /// Apply one offer. Text never participates in the removal decision.
    pub fn offer(&mut self, offer: SpanOffer) -> SpanOfferVerdict {
        let range = &offer.identity.range;
        let request_id = offer.identity.request_id;

        if !offer.decode_ok {
            return self.fence(NonContentEvidence::DecodeFailure, request_id, range.clone());
        }

        // Sealed identity is immutable — SessionIngest rule 5, keyed by range.
        if self.sealed_ranges.contains(&RangeKey::from_range(range)) {
            return self.reject_replay(
                NonContentEvidence::ReplayedRangeIdentity,
                request_id,
                range.clone(),
            );
        }

        if let Some(previous) = self
            .accepted
            .iter()
            .rev()
            .find(|span| span.identity.request_id == request_id)
        {
            // In-span loop: same request, clock did not move. Not a content check.
            if !offer.timestamps_progressed
                || (range.sample_end <= previous.identity.range.sample_end
                    && range.sample_start <= previous.identity.range.sample_start)
            {
                return self.fence(
                    NonContentEvidence::NonProgressingTimestamps,
                    request_id,
                    range.clone(),
                );
            }
            return self.reject_replay(
                NonContentEvidence::ReplayedRequestIdentity,
                request_id,
                range.clone(),
            );
        }

        let content_similar = self
            .accepted
            .iter()
            .any(|span| span.text == offer.text && !offer.text.trim().is_empty());

        self.accepted.push(AcceptedSpan {
            identity: offer.identity.clone(),
            text: offer.text,
        });

        if content_similar {
            self.receipts.push(SpanIdempotenceReceipt {
                code: SpanOfferVerdict::WarnPreserved.as_token(),
                warn: true,
                request_id,
                range: range.clone(),
            });
            return SpanOfferVerdict::WarnPreserved;
        }

        SpanOfferVerdict::Accepted
    }

    /// Record that a range has sealed (immutable). Later exact-identity
    /// offers are `RejectedSealedReplay` even if the Apple id is new.
    pub fn mark_sealed(&mut self, range: &TailSampleRange) {
        self.sealed_ranges.insert(RangeKey::from_range(range));
    }

    pub fn receipts(&self) -> &[SpanIdempotenceReceipt] {
        &self.receipts
    }

    #[cfg(test)]
    fn canvas_texts(&self) -> Vec<&str> {
        self.accepted
            .iter()
            .map(|span| span.text.as_str())
            .collect()
    }

    #[cfg(test)]
    fn canvas(&self) -> String {
        self.canvas_texts().join(" ")
    }

    #[cfg(test)]
    fn warn_count(&self) -> usize {
        self.receipts.iter().filter(|receipt| receipt.warn).count()
    }

    #[cfg(test)]
    fn suppressed_count(&self) -> usize {
        self.receipts
            .iter()
            .filter(|receipt| {
                matches!(
                    receipt.code,
                    "replayed_range_identity"
                        | "replayed_request_identity"
                        | "non_progressing_timestamps"
                        | "decode_failure"
                )
            })
            .count()
    }

    #[cfg(test)]
    fn verdict_warns(&self) -> usize {
        self.receipts
            .iter()
            .filter(|receipt| receipt.code == "content_similar_preserved")
            .count()
    }

    fn reject_replay(
        &mut self,
        evidence: NonContentEvidence,
        request_id: u64,
        range: TailSampleRange,
    ) -> SpanOfferVerdict {
        self.receipts.push(SpanIdempotenceReceipt {
            code: evidence.as_str(),
            warn: false,
            request_id,
            range,
        });
        SpanOfferVerdict::RejectedSealedReplay
    }

    fn fence(
        &mut self,
        evidence: NonContentEvidence,
        request_id: u64,
        range: TailSampleRange,
    ) -> SpanOfferVerdict {
        self.receipts.push(SpanIdempotenceReceipt {
            code: evidence.as_str(),
            warn: false,
            request_id,
            range,
        });
        SpanOfferVerdict::FencedLoop { evidence }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTENCE: &str = "wpierdalało zabierało kradło";
    const RATE: u64 = 16_000;
    /// Silero-sized pause used by the 5× fixture (above [`super::super::silero_fusion::LONG_SILENCE_FENCE_SECS`]).
    const PAUSE_SAMPLES: u64 = RATE; // 1.0 s
    const UTTERANCE_SAMPLES: u64 = 8_000;

    fn range(start: u64, end: u64) -> TailSampleRange {
        TailSampleRange {
            session: "w13-4".into(),
            capture_epoch: 1,
            sample_start: start,
            sample_end: end,
        }
    }

    fn identity(request_id: u64, start: u64, end: u64) -> TailRequestIdentity {
        TailRequestIdentity {
            request_id,
            range: range(start, end),
        }
    }

    fn offer(
        request_id: u64,
        start: u64,
        end: u64,
        text: &str,
        timestamps_progressed: bool,
        decode_ok: bool,
    ) -> SpanOffer {
        SpanOffer {
            identity: identity(request_id, start, end),
            text: text.to_string(),
            timestamps_progressed,
            decode_ok,
        }
    }

    #[test]
    fn lane_defaults_off() {
        assert!(!lane_enabled_from_raw(None));
        assert!(!lane_enabled_from_raw(Some("off")));
        assert!(lane_enabled_from_raw(Some("on")));
    }

    #[test]
    fn decode_failure_is_fenced_without_canvas_write() {
        let mut ledger = SpanIdempotenceLedger::default();
        let verdict = ledger.offer(offer(1, 0, 8_000, SENTENCE, true, false));
        assert_eq!(
            verdict,
            SpanOfferVerdict::FencedLoop {
                evidence: NonContentEvidence::DecodeFailure
            }
        );
        assert!(ledger.canvas().is_empty());
        assert_eq!(ledger.receipts()[0].code, "decode_failure");
    }

    #[test]
    fn w13_span_idempotence_preserves_repetition() {
        // Fixture A — duplicate-once: same range identity replayed after seal.
        let mut duplicate = SpanIdempotenceLedger::default();
        assert_eq!(
            duplicate.offer(offer(
                10,
                0,
                UTTERANCE_SAMPLES,
                "fragment odzyskany",
                true,
                true
            )),
            SpanOfferVerdict::Accepted
        );
        duplicate.mark_sealed(&range(0, UTTERANCE_SAMPLES));
        let replay = duplicate.offer(offer(
            11,
            0,
            UTTERANCE_SAMPLES,
            "fragment odzyskany",
            true,
            true,
        ));
        assert_eq!(replay, SpanOfferVerdict::RejectedSealedReplay);
        assert_eq!(duplicate.canvas_texts(), ["fragment odzyskany"]);
        assert_eq!(duplicate.suppressed_count(), 1);
        assert_eq!(
            duplicate.receipts().last().map(|r| r.code),
            Some("replayed_range_identity")
        );

        // Same request_id re-submitted with a frozen clock is an in-span loop.
        let mut looped = SpanIdempotenceLedger::default();
        assert!(
            looped
                .offer(offer(3, 0, UTTERANCE_SAMPLES, SENTENCE, true, true))
                .lands_on_canvas()
        );
        let fenced = looped.offer(offer(3, 0, UTTERANCE_SAMPLES, SENTENCE, false, true));
        assert_eq!(
            fenced,
            SpanOfferVerdict::FencedLoop {
                evidence: NonContentEvidence::NonProgressingTimestamps
            }
        );
        assert_eq!(looped.canvas_texts(), [SENTENCE]);

        // Fixture B — 5× paused deliberate repetition (Silero-sized gaps).
        let mut paused = SpanIdempotenceLedger::default();
        let mut cursor = 0u64;
        let mut paused_verdicts = Vec::new();
        for i in 0..5u64 {
            let start = cursor;
            let end = start + UTTERANCE_SAMPLES;
            let verdict = paused.offer(offer(100 + i, start, end, SENTENCE, true, true));
            paused.mark_sealed(&range(start, end));
            paused_verdicts.push(verdict);
            cursor = end + PAUSE_SAMPLES;
        }
        assert!(
            paused_verdicts
                .iter()
                .all(SpanOfferVerdict::lands_on_canvas),
            "paused 5× must all land: {paused_verdicts:?}"
        );
        assert_eq!(paused.canvas_texts().len(), 5);
        assert_eq!(
            paused
                .canvas_texts()
                .iter()
                .filter(|t| **t == SENTENCE)
                .count(),
            5
        );
        assert_eq!(paused.verdict_warns(), 4);

        // Fixture C — continuous repetition, no Silero-sized gap, progressing clock.
        let mut continuous = SpanIdempotenceLedger::default();
        let mut cursor = 0u64;
        let mut continuous_verdicts = Vec::new();
        for i in 0..5u64 {
            let start = cursor;
            let end = start + UTTERANCE_SAMPLES;
            let verdict = continuous.offer(offer(200 + i, start, end, SENTENCE, true, true));
            continuous.mark_sealed(&range(start, end));
            continuous_verdicts.push(verdict);
            cursor = end; // abutting — no pause
        }
        assert!(
            continuous_verdicts
                .iter()
                .all(SpanOfferVerdict::lands_on_canvas),
            "continuous 5× must all land: {continuous_verdicts:?}"
        );
        assert_eq!(continuous.canvas_texts().len(), 5);
        assert_eq!(
            continuous
                .canvas_texts()
                .iter()
                .filter(|t| **t == SENTENCE)
                .count(),
            5
        );
        assert_eq!(continuous.verdict_warns(), 4);

        // Choice rule: if content looks like a duplicate but identity is new,
        // repetition wins and the would-be drop is a WARN receipt.
        assert!(paused.warn_count() >= 4);
        assert!(continuous.warn_count() >= 4);
        assert_eq!(paused.suppressed_count(), 0);
        assert_eq!(continuous.suppressed_count(), 0);
    }
}
