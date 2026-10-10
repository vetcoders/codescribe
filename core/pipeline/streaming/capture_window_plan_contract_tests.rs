// Integrator-owned architectural contracts. No recognizer accuracy claims.
use super::CaptureWindowPlan;
use crate::stt::tail_provider::TailSampleRange;

fn plan(rate: u32) -> CaptureWindowPlan {
    CaptureWindowPlan::new("capture-plan-proof".into(), 4, rate)
}

fn collect(plan: &mut CaptureWindowPlan, end: u64, stop: bool) -> Vec<TailSampleRange> {
    let mut out = Vec::new();
    while let Some(range) = plan.next_due(end, stop) {
        assert!(plan.account(&range), "offered work must be account-able");
        out.push(range);
        assert!(out.len() < 100, "work plan must advance");
    }
    out
}

fn bounds(ranges: &[TailSampleRange]) -> Vec<(u64, u64)> {
    ranges
        .iter()
        .map(|r| (r.sample_start, r.sample_end))
        .collect()
}

#[test]
fn fragmented_ingress_and_single_ingress_produce_the_same_pcm_work() {
    let mut fragmented = plan(100);
    let mut ranges = Vec::new();
    for end in [
        1, 173, 600, 899, 900, 933, 1199, 1200, 1781, 2100, 2499, 2500,
    ] {
        ranges.extend(collect(&mut fragmented, end, false));
    }
    ranges.extend(collect(&mut fragmented, 2500, true));
    let once = collect(&mut plan(100), 2500, true);
    assert_eq!(ranges, once);
    assert_eq!(
        bounds(&once),
        vec![
            (0, 900),
            (300, 1200),
            (600, 1500),
            (900, 1800),
            (1200, 2100),
            (1500, 2400),
            (1800, 2500)
        ]
    );
    let mut visits = vec![0_u8; 2500];
    for range in &once {
        assert_eq!(range.session, "capture-plan-proof");
        assert_eq!(range.capture_epoch, 4);
        for sample in range.sample_start..range.sample_end {
            visits[sample as usize] += 1;
        }
    }
    assert!(visits.iter().all(|n| (1..=3).contains(n)));
    assert_eq!(visits.iter().max(), Some(&3));
}

#[test]
fn exact_full_window_eof_has_no_repeated_tail_and_short_take_has_one_tail() {
    let mut exact = plan(100);
    let windows = collect(&mut exact, 2400, true);
    assert_eq!(windows.len(), 6);
    assert_eq!(bounds(&windows).last(), Some(&(1500, 2400)));
    assert!(exact.next_due(2400, true).is_none());
    assert!(exact.next_due(2400, false).is_none());
    for end in [1, 299, 300, 899] {
        let mut short = plan(100);
        assert!(short.next_due(end, false).is_none());
        assert_eq!(bounds(&collect(&mut short, end, true)), vec![(0, end)]);
        assert!(short.next_due(end, true).is_none());
    }
}

#[test]
fn blocked_transport_does_not_discard_reanchor_or_duplicate_due_work() {
    let mut owner = plan(100);
    let first = owner.next_due(900, false).unwrap();
    assert_eq!(owner.next_due(1700, false), Some(first.clone()));
    assert_eq!(owner.next_due(1700, true), Some(first.clone()));
    assert!(owner.account(&first));
    // An accepted call that fails has already consumed this work. No rewind.
    assert!(!owner.account(&first));
    assert_eq!(
        bounds(&collect(&mut owner, 1700, true)),
        vec![(300, 1200), (600, 1500), (900, 1700)]
    );
    assert!(owner.next_due(1700, true).is_none());
}

#[test]
fn frozen_eof_drains_queued_work_despite_later_counter_changes() {
    let mut owner = plan(100);
    let first = owner.next_due(900, false).unwrap();
    assert_eq!(owner.next_due(1700, true), Some(first.clone()));
    assert_eq!(owner.next_due(2500, true), Some(first.clone()));
    assert!(owner.account(&first));
    assert_eq!(
        bounds(&collect(&mut owner, 2500, true)),
        vec![(300, 1200), (600, 1500), (900, 1700)]
    );
    assert!(owner.is_finished());
    assert_eq!(owner.admission_horizon(), u64::MAX);
}

#[test]
fn regressed_stop_head_cannot_revoke_the_outstanding_offer() {
    let mut owner = plan(100);
    let first = owner.next_due(900, false).unwrap();
    assert!(owner.next_due(899, true).is_none());
    assert_eq!(owner.next_due(900, true), Some(first.clone()));
    assert!(owner.account(&first));
    assert!(owner.next_due(1200, true).is_none());
    assert!(owner.is_finished());
}

#[test]
fn foreign_wrong_unoffered_and_stale_coordinates_cannot_advance_the_plan() {
    let mut owner = plan(100);
    let candidate = TailSampleRange {
        session: "capture-plan-proof".into(),
        capture_epoch: 4,
        sample_start: 0,
        sample_end: 900,
    };
    assert!(!owner.account(&candidate));
    assert_eq!(owner.next_due(900, false), Some(candidate.clone()));
    let mut foreign = candidate.clone();
    foreign.capture_epoch += 1;
    assert!(!owner.account(&foreign));
    foreign = candidate.clone();
    foreign.session = "different-capture".into();
    assert!(!owner.account(&foreign));
    foreign = candidate.clone();
    foreign.sample_start += 1;
    assert!(!owner.account(&foreign));
    assert!(owner.next_due(899, false).is_none());
    assert_eq!(owner.next_due(900, false), Some(candidate.clone()));
    assert!(owner.account(&candidate));
    assert!(owner.next_due(900, true).is_none());
    assert!(
        owner.next_due(1200, true).is_none(),
        "EOF cannot reopen capture"
    );
}

#[test]
fn grid_uses_original_sample_rate_and_preserves_half_open_three_visit_bound() {
    for rate in [1, 16000, 44100, 48000] {
        let mut owner = plan(rate);
        let r = u64::from(rate);
        let windows = collect(&mut owner, 25 * r + 1, true);
        assert_eq!(windows.len(), 7);
        for (index, range) in windows.iter().enumerate() {
            assert_eq!(range.sample_start, index as u64 * 3 * r);
            assert_eq!(
                range.sample_end,
                (range.sample_start + 9 * r).min(25 * r + 1)
            );
        }
        for sample in [
            0,
            3 * r - 1,
            3 * r,
            6 * r,
            9 * r - 1,
            9 * r,
            18 * r,
            24 * r,
            25 * r,
        ] {
            let visits = windows
                .iter()
                .filter(|range| range.sample_start <= sample && sample < range.sample_end)
                .count();
            assert!((1..=3).contains(&visits), "rate {rate}, sample {sample}");
        }
    }
    assert!(plan(0).next_due(100, true).is_none());
}

#[test]
fn bounded_trial_keeps_grid_cursor_and_spends_one_capture_bucket() {
    let mut owner = plan(100);
    collect(&mut owner, 1800, false);
    let horizon = owner.admission_horizon();
    let trial = owner.offer_trial(800, 1450, 1800, false).unwrap();
    assert_eq!(owner.next_due(1800, false), Some(trial.clone()));
    assert!(owner.account(&trial));
    assert_eq!(owner.admission_horizon(), horizon);
    assert!(owner.offer_trial(850, 1500, 1800, false).is_none());
    assert!(owner.next_due(1800, false).is_none());
    assert!(!owner.account(&trial));
    collect(&mut owner, 2700, false);
    assert!(owner.offer_trial(1800, 2450, 2700, false).is_some());
}

#[test]
fn bounded_trial_cannot_reopen_early_pcm_at_stop_or_exceed_window_budget() {
    let mut owner = plan(100);
    collect(&mut owner, 2700, true);
    assert!(owner.offer_trial(1000, 1600, 2700, true).is_none());
    assert!(owner.offer_trial(1799, 2700, 2700, true).is_none());
    assert!(owner.offer_trial(1800, 2701, 2700, true).is_none());
    let tail = owner.offer_trial(1850, 2650, 2700, true).unwrap();
    assert!(!owner.is_finished());
    assert!(owner.account(&tail));
    assert!(owner.is_finished());
    assert!(owner.next_due(4000, true).is_none());
}
