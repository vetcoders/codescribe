use super::{EpochDecision, EpochGate};

#[test]
fn silence_seals_one_epoch_and_the_gate_stays_armed() {
    let mut gate = EpochGate::for_session(16_000, Some(0.2), true);
    assert!(gate.is_armed());
    let speech = vec![0.2_f32; 160];
    let silence = vec![0.0_f32; 160];
    assert!(matches!(
        gate.feed_pcm(&speech, 160, true),
        EpochDecision::Wake { .. }
    ));
    let mut seen = 160_u64;
    let mut slept = false;
    for _ in 0..40 {
        seen += 160;
        if matches!(
            gate.feed_pcm(&silence, seen, false),
            EpochDecision::Sleep { .. }
        ) {
            slept = true;
            break;
        }
    }
    assert!(slept, "configured silence must close the utterance epoch");
    assert!(
        gate.is_armed(),
        "the channel gate stays armed after the seal"
    );
    assert!(matches!(
        gate.feed_pcm(&speech, seen + 160, true),
        EpochDecision::Wake { .. }
    ));
}
