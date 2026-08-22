//! Acoustic span identity versus quality evidence.
//!
//! Identity is structural: session, capture epoch, half-open PCM range, order.
//! Energy hops (`session_energy_db`) are evidence on that axis — never a hash
//! and never a collision-proof ID. Mean dBFS of a window is not identity.
//!
//! Text equality is not identity. Five disjoint "Iwo" spans are five
//! observations. Replaying one range must not mint a sixth.

use crate::pipeline::contracts::AcousticTranscriptSpan;
use crate::stt::tail_provider::TailSampleRange;

/// How two PCM ranges relate. Text is not an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcousticSpanRelation {
    /// Exact same samples. A second decode is overlap replay.
    Same,
    /// Shared samples, not identical. Layer 1 ~1 s overlap: keep exclusive tail.
    Overlapping,
    /// Same epoch, no shared samples. Both observations must survive.
    Disjoint,
    /// Zero-length or inverted range. No mutation authority.
    Unanchored,
    /// Different session or capture epoch.
    DifferentEpoch,
}

/// One-to-one receipt for preserve / correct / refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcousticAdmitReceipt {
    Preserve,
    Correct { clipped_start: u64 },
    RefuseReplay,
    RefuseUnanchored,
    RefuseDifferentEpoch,
}

/// Quality evidence for a range. Not part of identity. Not Eq/Hash of the span.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcousticQualityEvidence {
    pub energy_db: Option<f32>,
}

/// Relate two capture ranges. Energy is ignored on purpose.
pub fn relate(left: &TailSampleRange, right: &TailSampleRange) -> AcousticSpanRelation {
    if left.sample_end <= left.sample_start || right.sample_end <= right.sample_start {
        return AcousticSpanRelation::Unanchored;
    }
    if left.session != right.session || left.capture_epoch != right.capture_epoch {
        return AcousticSpanRelation::DifferentEpoch;
    }
    if left.sample_start == right.sample_start && left.sample_end == right.sample_end {
        return AcousticSpanRelation::Same;
    }
    if left.overlaps(right) {
        return AcousticSpanRelation::Overlapping;
    }
    AcousticSpanRelation::Disjoint
}

/// Mean energy is quality evidence. It never decides Same vs Disjoint.
pub fn mean_energy_is_identity(_evidence: AcousticQualityEvidence) -> bool {
    false
}

/// Admit incoming Layer-1 / overlap observations against already-committed spans.
///
/// Disjoint identical text is preserved. Exact-range replay is refused.
/// Overlap keeps only samples strictly after the latest overlapping committed end.
pub fn admit_acoustic_spans(
    committed: &[AcousticTranscriptSpan],
    incoming: &[AcousticTranscriptSpan],
) -> (Vec<AcousticTranscriptSpan>, Vec<AcousticAdmitReceipt>) {
    let mut admitted = Vec::new();
    let mut receipts = Vec::new();
    for span in incoming {
        match admit_one(committed, span) {
            Ok((kept, receipt)) => {
                admitted.push(kept);
                receipts.push(receipt);
            }
            Err(receipt) => receipts.push(receipt),
        }
    }
    (admitted, receipts)
}

fn admit_one(
    committed: &[AcousticTranscriptSpan],
    incoming: &AcousticTranscriptSpan,
) -> Result<(AcousticTranscriptSpan, AcousticAdmitReceipt), AcousticAdmitReceipt> {
    if incoming.range.sample_end <= incoming.range.sample_start {
        return Err(AcousticAdmitReceipt::RefuseUnanchored);
    }
    let mut clip_start = incoming.range.sample_start;
    for prior in committed {
        match relate(&prior.range, &incoming.range) {
            AcousticSpanRelation::Unanchored => {
                return Err(AcousticAdmitReceipt::RefuseUnanchored);
            }
            AcousticSpanRelation::DifferentEpoch => {
                return Err(AcousticAdmitReceipt::RefuseDifferentEpoch);
            }
            AcousticSpanRelation::Same => return Err(AcousticAdmitReceipt::RefuseReplay),
            AcousticSpanRelation::Overlapping => {
                clip_start = clip_start.max(prior.range.sample_end);
            }
            AcousticSpanRelation::Disjoint => {}
        }
    }
    if clip_start >= incoming.range.sample_end {
        return Err(AcousticAdmitReceipt::RefuseReplay);
    }
    if clip_start == incoming.range.sample_start {
        return Ok((incoming.clone(), AcousticAdmitReceipt::Preserve));
    }
    let mut clipped = incoming.clone();
    clipped.range.sample_start = clip_start;
    Ok((
        clipped,
        AcousticAdmitReceipt::Correct {
            clipped_start: clip_start,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contracts::AcousticSpanGrain;

    fn range(start: u64, end: u64) -> TailSampleRange {
        TailSampleRange {
            session: "take".into(),
            capture_epoch: 1,
            sample_start: start,
            sample_end: end,
        }
    }

    fn iwo(start: u64, end: u64) -> AcousticTranscriptSpan {
        AcousticTranscriptSpan {
            text: "Iwo".into(),
            range: range(start, end),
            grain: AcousticSpanGrain::Word,
        }
    }

    #[test]
    fn five_iwo_on_five_pcm_ranges_all_survive() {
        let incoming: Vec<_> = (0..5).map(|i| iwo(i * 1600, i * 1600 + 1600)).collect();
        let (admitted, receipts) = admit_acoustic_spans(&[], &incoming);
        assert_eq!(admitted.len(), 5);
        assert!(
            receipts
                .iter()
                .all(|receipt| *receipt == AcousticAdmitReceipt::Preserve)
        );
        assert!(admitted.iter().all(|span| span.text == "Iwo"));
    }

    #[test]
    fn replaying_one_range_does_not_mint_a_sixth() {
        let first = iwo(0, 1600);
        let (committed, _) = admit_acoustic_spans(&[], &[first.clone()]);
        let (admitted, receipts) = admit_acoustic_spans(&committed, &[first]);
        assert!(admitted.is_empty());
        assert_eq!(receipts, [AcousticAdmitReceipt::RefuseReplay]);
    }

    #[test]
    fn overlap_window_keeps_exclusive_tail_only() {
        let committed = [iwo(0, 48_000)];
        let incoming = AcousticTranscriptSpan {
            text: "Iwo later".into(),
            range: range(32_000, 64_000),
            grain: AcousticSpanGrain::Phrase,
        };
        let (admitted, receipts) = admit_acoustic_spans(&committed, &[incoming]);
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].range.sample_start, 48_000);
        assert_eq!(admitted[0].range.sample_end, 64_000);
        assert_eq!(
            receipts,
            [AcousticAdmitReceipt::Correct {
                clipped_start: 48_000
            }]
        );
    }

    #[test]
    fn unanchored_range_has_no_mutation_right() {
        let empty = AcousticTranscriptSpan {
            text: "Iwo".into(),
            range: range(8, 8),
            grain: AcousticSpanGrain::Word,
        };
        let (admitted, receipts) = admit_acoustic_spans(&[], &[empty]);
        assert!(admitted.is_empty());
        assert_eq!(receipts, [AcousticAdmitReceipt::RefuseUnanchored]);
    }

    #[test]
    fn mean_energy_never_decides_identity() {
        assert!(!mean_energy_is_identity(AcousticQualityEvidence {
            energy_db: Some(-18.0),
        }));
        assert_eq!(
            relate(&range(0, 1600), &range(1600, 3200)),
            AcousticSpanRelation::Disjoint
        );
        assert_eq!(
            relate(&range(0, 1600), &range(0, 1600)),
            AcousticSpanRelation::Same
        );
    }

    #[test]
    fn string_suffix_dedup_is_the_forbidden_path_for_anchored_spans() {
        let incoming: Vec<_> = (0..5).map(|i| iwo(i * 1600, i * 1600 + 1600)).collect();
        let (admitted, _) = admit_acoustic_spans(&[], &incoming);
        let collapsed = crate::pipeline::dedup::strip_suffix_overlap_live(
            "Iwo Iwo Iwo Iwo",
            "Iwo Iwo Iwo Iwo Iwo",
        );
        assert_ne!(
            collapsed.split_whitespace().count(),
            5,
            "text overlap is the bug this module exists to refuse"
        );
        assert_eq!(admitted.len(), 5);
    }
}
