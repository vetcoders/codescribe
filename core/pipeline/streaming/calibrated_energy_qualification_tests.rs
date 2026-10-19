//! Integrator regressions for the production occurrence qualification path.
//! Synthetic capture proves admission geometry, not recognition accuracy.

use super::{AppleSealState, EnergyCalibration, OccurrenceIdentity, qualify_owned_occurrence};
use crate::audio::capture_receipt::CaptureLevelAccumulator;

const RATE: u32 = 1_000;
const FLOOR: f32 = 0.05;

fn capture(session: &str, pcm: &[f32]) -> AppleSealState {
    let mut state = AppleSealState::new_for_session(RATE, session.into(), 1);
    state.existence_floor_rms = Some(FLOOR);
    state.energy_calibration = Some(EnergyCalibration::new(
        "measured-floor-regression",
        f64::from(FLOOR).powi(2) * 64.0,
        20,
    ));
    state.audio.push(pcm);
    let mut capture = CaptureLevelAccumulator::bound_to_at_rate(&state.capture_energy, RATE);
    capture.push_samples(pcm);
    state
}

#[test]
fn long_sub_floor_noise_cannot_accumulate_into_an_occurrence() {
    let state = capture("sub-floor-noise", &[0.04; 1_000]);
    let owner = OccurrenceIdentity::new("sub-floor-noise", 1, 0, 1_000);

    assert!(!qualify_owned_occurrence(&state, &owner));
    assert!(!state.acoustic_ledger.lock().unwrap().is_qualified(&owner));
}

#[test]
fn short_burst_does_not_borrow_energy_from_background_noise() {
    let mut pcm = vec![0.04; 1_000];
    pcm[..20].fill(0.06);
    let state = capture("short-burst", &pcm);
    let owner = OccurrenceIdentity::new("short-burst", 1, 0, 1_000);

    assert!(!qualify_owned_occurrence(&state, &owner));
}

#[test]
fn voice_outside_the_owned_span_cannot_qualify_its_noise() {
    let mut pcm = vec![0.04; 1_000];
    pcm[..80].fill(0.06);
    let state = capture("outside-voice", &pcm);
    let owner = OccurrenceIdentity::new("outside-voice", 1, 100, 500);

    assert!(!qualify_owned_occurrence(&state, &owner));
}

#[test]
fn repeated_real_bursts_keep_both_physical_occurrences() {
    let mut pcm = vec![0.04; 1_000];
    pcm[..80].fill(0.06);
    pcm[500..580].fill(0.06);
    let state = capture("two-real-bursts", &pcm);
    let first = OccurrenceIdentity::new("two-real-bursts", 1, 0, 500);
    let second = OccurrenceIdentity::new("two-real-bursts", 1, 500, 1_000);

    assert!(qualify_owned_occurrence(&state, &first));
    assert!(qualify_owned_occurrence(&state, &second));
    let ledger = state.acoustic_ledger.lock().unwrap();
    assert!(ledger.serial_of(&first).is_some());
    assert!(ledger.serial_of(&second).is_some());
}
