// Integrator-authored contracts, installed as a private module after admission.
use super::*;

fn range(start: u64, end: u64) -> TailSampleRange {
    TailSampleRange {
        session: "budget-proof".into(),
        capture_epoch: 4,
        sample_start: start,
        sample_end: end,
    }
}

#[test]
fn live_reserves_the_third_pass_for_terminal_and_failure_is_not_refunded() {
    let mut budget = CaptureDecodeBudget::new("budget-proof".into(), 4);
    let r = range(10, 100);
    assert!(budget.record(&r, DecodePhase::Live));
    // A failed accepted attempt is still physical work: no success is needed.
    assert!(budget.record(&r, DecodePhase::Live));
    assert!(!budget.can_admit(&r, DecodePhase::Live));
    assert!(!budget.record(&r, DecodePhase::Live));
    assert!(budget.record(&r, DecodePhase::Terminal));
    assert!(!budget.record(&r, DecodePhase::Terminal));
    assert_eq!(budget.maximum(10, 100), 3);
}

#[test]
fn partial_overlap_never_exceeds_three_and_fresh_pcm_keeps_its_capacity() {
    let mut budget = CaptureDecodeBudget::new("budget-proof".into(), 4);
    assert!(budget.record(&range(0, 100), DecodePhase::Live));
    assert!(budget.record(&range(50, 150), DecodePhase::Live));
    assert!(!budget.record(&range(75, 175), DecodePhase::Live));
    assert!(budget.record(&range(100, 175), DecodePhase::Live));
    assert!(budget.record(&range(0, 175), DecodePhase::Terminal));
    assert_eq!(budget.maximum(0, 175), 3);
    assert!(!budget.can_admit(&range(99, 101), DecodePhase::Terminal));
    assert!(budget.can_admit(&range(175, 200), DecodePhase::Live));
}

#[test]
fn old_counts_survive_horizon_advance_and_other_capture_is_refused() {
    let mut budget = CaptureDecodeBudget::new("budget-proof".into(), 4);
    let old = range(0, 100);
    for phase in [DecodePhase::Live, DecodePhase::Live, DecodePhase::Terminal] {
        assert!(budget.record(&old, phase));
    }
    budget.advance_live_horizon(1_000, 100);
    assert!(!budget.can_admit(&old, DecodePhase::Live));
    assert!(!budget.can_admit(&old, DecodePhase::Terminal));
    let mut foreign = range(1_000, 1_100);
    foreign.capture_epoch += 1;
    assert!(!budget.record(&foreign, DecodePhase::Live));
    foreign.capture_epoch = 4;
    foreign.session = "another-session".into();
    assert!(!budget.record(&foreign, DecodePhase::Terminal));
    assert!(!budget.record(&range(0, 0), DecodePhase::Terminal));
}

#[test]
fn available_ranges_exclude_saturated_pcm_without_discarding_new_samples() {
    let mut budget = CaptureDecodeBudget::new("budget-proof".into(), 4);
    let middle = range(50, 100);
    for phase in [DecodePhase::Live, DecodePhase::Live, DecodePhase::Terminal] {
        assert!(budget.record(&middle, phase));
    }
    let result = budget.available_ranges(&range(0, 150), DecodePhase::Terminal);
    assert_eq!(result, vec![range(0, 50), range(100, 150)]);
}

#[test]
fn terminal_window_union_avoids_redecoding_intersecting_debt() {
    let result = terminal_decode_windows(vec![range(0, 150), range(50, 200), range(300, 350)], 90);
    assert!(result.iter().all(|r| r.sample_end - r.sample_start <= 90));
    let mut seen = [0_u8; 350];
    for r in result {
        for sample in r.sample_start..r.sample_end {
            seen[sample as usize] += 1;
        }
    }
    for (i, count) in seen.into_iter().enumerate() {
        assert_eq!(count, u8::from(i < 200 || i >= 300), "sample {i}");
    }
}
