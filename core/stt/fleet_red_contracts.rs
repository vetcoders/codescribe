//! Precommitted RED witnesses for Layer 1 surfaces that do not exist yet.
//!
//! These probes deliberately stop at a typed `MissingContract` boundary. They
//! contain no provider, process, network, model, sleep, audio, or runtime code.
//! Follow-on cuts replace each probe with the production seam named by the test;
//! until then every failure identifies exactly which architectural contract is
//! absent instead of collapsing into a generic compile error.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingContract {
    TypedMonotonicEvents,
    BoundedAppleOnlyDegradation,
    ExplicitAudioEgressConsent,
    KillableLocalHelper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudSessionError {
    ConsentRequired,
    Missing(MissingContract),
}

fn missing<T>(contract: MissingContract) -> Result<T, MissingContract> {
    Err(contract)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EventIdentity {
    session_id: &'static str,
    utterance_id: u64,
    sequence_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AsrSessionEvent {
    Partial(EventIdentity),
    Final(EventIdentity),
    Error(EventIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EventIngestOutcome {
    accepted: Vec<AsrSessionEvent>,
    duplicate_final_was_idempotent: bool,
    out_of_order_final_was_rejected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackpressureOutcome {
    capture_continued: bool,
    apple_only: bool,
    overflow_degraded: bool,
    disconnect_degraded: bool,
    whisper_init_calls: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HelperExitOutcome {
    child_pid_observed: bool,
    child_exited: bool,
    gui_model_fallback_loaded: bool,
}

#[test]
fn fleet_red_asr_session_events_are_typed_and_monotonic() {
    let id = |sequence_id| EventIdentity {
        session_id: "session-a",
        utterance_id: 7,
        sequence_id,
    };
    let inputs = [
        AsrSessionEvent::Partial(id(1)),
        AsrSessionEvent::Final(id(2)),
        AsrSessionEvent::Final(id(2)), // duplicate: idempotent
        AsrSessionEvent::Final(id(1)), // out of order: rejected
        AsrSessionEvent::Error(id(3)),
    ];
    assert_eq!(
        inputs.len(),
        5,
        "contract fixture must exercise both reorder cases"
    );

    let outcome: Result<EventIngestOutcome, _> = missing(MissingContract::TypedMonotonicEvents);
    assert_eq!(
        outcome,
        Ok(EventIngestOutcome {
            accepted: vec![
                AsrSessionEvent::Partial(id(1)),
                AsrSessionEvent::Final(id(2)),
                AsrSessionEvent::Error(id(3)),
            ],
            duplicate_final_was_idempotent: true,
            out_of_order_final_was_rejected: true,
        }),
        "partial/final/error events must carry session, utterance, and sequence identity; duplicate or out-of-order finals must be idempotent or rejected"
    );
}

#[test]
fn fleet_red_cloud_backpressure_degrades_to_apple_only() {
    let outcome: Result<BackpressureOutcome, _> =
        missing(MissingContract::BoundedAppleOnlyDegradation);
    assert_eq!(
        outcome,
        Ok(BackpressureOutcome {
            capture_continued: true,
            apple_only: true,
            overflow_degraded: true,
            disconnect_degraded: true,
            whisper_init_calls: 0,
        }),
        "bounded overflow or disconnect must not block Apple capture or initialize Whisper"
    );
}

#[test]
fn fleet_red_cloud_requires_explicit_consent() {
    let session_without_consent: Result<(), CloudSessionError> = Err(CloudSessionError::Missing(
        MissingContract::ExplicitAudioEgressConsent,
    ));
    assert_eq!(
        session_without_consent,
        Err(CloudSessionError::ConsentRequired),
        "session construction without explicit audio-egress consent must be rejected by the production factory"
    );
}

#[test]
fn fleet_red_local_helper_exit_reclaims_process() {
    let outcome: Result<HelperExitOutcome, _> = missing(MissingContract::KillableLocalHelper);
    assert_eq!(
        outcome,
        Ok(HelperExitOutcome {
            child_pid_observed: true,
            child_exited: true,
            gui_model_fallback_loaded: false,
        }),
        "fake helper shutdown must prove process exit and no hidden in-GUI model fallback"
    );
}
