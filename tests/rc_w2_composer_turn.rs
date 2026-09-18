//! rc-w2-composer-turn: the per-take capture intent, proved on the production
//! type that the recorder, the session and the controller all read.
//!
//! These are contract assertions, not a behavioural receipt. Whether the
//! composer take actually stays open through two long pauses, and whether the
//! provider is charged exactly once, is a W4 question answered by a real take —
//! source tests cannot decide it and must not be read as if they did.

#![cfg(target_os = "macos")]

use codescribe_core::audio::streaming_recorder::CaptureTurnIntent;

/// The configured hands-free threshold used across these assertions.
const CONFIGURED_SILENCE_SEC: f32 = 2.5;

#[test]
fn a_one_turn_take_asks_for_no_utterance_epochs() {
    assert_eq!(
        CaptureTurnIntent::SingleTurn.utterance_silence_sec(CONFIGURED_SILENCE_SEC),
        None,
        "silence must not close a take the user ends explicitly"
    );
}

#[test]
fn hands_free_takes_keep_the_configured_threshold() {
    assert_eq!(
        CaptureTurnIntent::HandsFree.utterance_silence_sec(CONFIGURED_SILENCE_SEC),
        Some(CONFIGURED_SILENCE_SEC),
        "hotkey, hold, tray and overlay keep the epoch contract they had"
    );
}

/// The one-turn override is expressed only as `None` for this take. It carries
/// no new global default and cannot rewrite the configured value it ignores.
#[test]
fn the_one_turn_override_never_rewrites_the_configured_value() {
    for configured in [0.5_f32, 2.5, 30.0] {
        assert_eq!(
            CaptureTurnIntent::SingleTurn.utterance_silence_sec(configured),
            None
        );
        assert_eq!(
            CaptureTurnIntent::HandsFree.utterance_silence_sec(configured),
            Some(configured),
            "the next hands-free take must read the setting, not a leaked override"
        );
    }
}

/// Formatting is charged exactly once per take, on exactly one of the two
/// lanes — never both, never neither.
#[test]
fn every_intent_pays_for_formatting_on_exactly_one_lane() {
    for intent in [CaptureTurnIntent::HandsFree, CaptureTurnIntent::SingleTurn] {
        assert_ne!(
            intent.schedules_live_formatting(),
            intent.formats_once_at_terminal(),
            "{intent:?} must format either while live or once at terminal, not both or neither"
        );
    }
    assert!(CaptureTurnIntent::HandsFree.schedules_live_formatting());
    assert!(CaptureTurnIntent::SingleTurn.formats_once_at_terminal());
}

/// Absence of an explicit intent is the pre-existing hands-free behaviour, so a
/// surface that never opts in cannot silently acquire one-turn semantics.
#[test]
fn the_default_intent_is_hands_free() {
    assert_eq!(CaptureTurnIntent::default(), CaptureTurnIntent::HandsFree);
}
