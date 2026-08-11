//! Precommitted RED witnesses for Layer 1 surfaces that do not exist yet.
//!
//! These probes deliberately stop at a typed `MissingContract` boundary. They
//! contain no provider, process, network, model, sleep, audio, or runtime code.
//! Follow-on cuts replace each probe with the production seam named by the test;
//! until then every failure identifies exactly which architectural contract is
//! absent instead of collapsing into a generic compile error.
//!
//! Promotion is what "done" looks like here: a probe stops being a placeholder
//! when it runs against the module it named. `TypedMonotonicEvents` was
//! promoted by the `p0-asr-session-contract` cut and now exercises
//! `crate::asr_session`; `BoundedAppleOnlyDegradation` was promoted by the
//! `c1-live-recorder-orchestration` cut and now measures
//! `crate::asr_session::recorder`. Deleting a `missing(...)` call without a
//! seam behind it would be the opposite of that.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingContract {
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

/// PROMOTED (P0, `p0-asr-session-contract`): this probe no longer stops at a
/// `MissingContract` boundary. The seam it named exists — `asr_session` — and
/// the same fixture now runs through the production ingest ledger. The depth
/// (sealed utterances, foreign sessions, bounded ranges, the fake provider,
/// canvas-versus-refiner separation) lives in `asr_session::tests`; what stays
/// here is the fleet-level witness that the fixture the wave precommitted is
/// the fixture production satisfies.
#[test]
fn fleet_red_asr_session_events_are_typed_and_monotonic() {
    use crate::asr_session::{
        AsrErrorKind, AsrSessionEvent, ErrorEvent, EventIdentity, IngestVerdict, SessionId,
        SessionIngest, TranscriptEvent,
    };

    let session = SessionId::new("session-a").expect("non-blank session id");
    let id = |sequence_id| EventIdentity::new(session.clone(), 7, sequence_id);
    let transcript = |sequence_id, text: &str| TranscriptEvent {
        identity: id(sequence_id),
        text: text.to_string(),
        range: None,
    };

    let inputs = vec![
        AsrSessionEvent::Partial(transcript(1, "pacjent ma")),
        AsrSessionEvent::Final(transcript(2, "pacjent ma goraczke")),
        AsrSessionEvent::Final(transcript(2, "pacjent ma goraczke")), // duplicate: idempotent
        AsrSessionEvent::Final(transcript(1, "pacjent")),             // out of order: rejected
        AsrSessionEvent::Error(ErrorEvent {
            identity: id(3),
            kind: AsrErrorKind::Transport,
        }),
    ];
    assert_eq!(
        inputs.len(),
        5,
        "contract fixture must exercise both reorder cases"
    );

    let mut ingest = SessionIngest::new(session);
    let verdicts: Vec<IngestVerdict> = inputs
        .into_iter()
        .map(|event| ingest.ingest(event))
        .collect();

    assert_eq!(
        verdicts,
        vec![
            IngestVerdict::Accepted,
            IngestVerdict::Accepted,
            IngestVerdict::DuplicateIdempotent,
            IngestVerdict::RejectedOutOfOrder,
            IngestVerdict::Accepted,
        ],
        "partial/final/error events must carry session, utterance, and sequence identity; duplicate or out-of-order finals must be idempotent or rejected"
    );
    let accepted: Vec<(&str, u64)> = ingest
        .accepted()
        .iter()
        .map(|event| (event.as_token(), event.identity().sequence_id()))
        .collect();
    assert_eq!(accepted, vec![("partial", 1), ("final", 2), ("error", 3)]);
    assert_eq!(
        ingest.duplicate_count(),
        1,
        "duplicate final was idempotent"
    );
    assert_eq!(
        ingest.out_of_order_count(),
        1,
        "out-of-order final was rejected"
    );
    assert_eq!(
        ingest.sealed_final(7).map(|event| event.text.as_str()),
        Some("pacjent ma goraczke"),
        "the stale final must not have rewritten committed text"
    );
}

/// PROMOTED (C1, `c1-live-recorder-orchestration`): this probe no longer stops
/// at a `MissingContract` boundary. The seam it named exists —
/// `asr_session::recorder::RecorderLayer1Lane` — and the outcome below is
/// *measured* against that production state machine, not declared: two lanes
/// run against injected failing providers (sustained overflow, transport
/// disconnect), capture keeps offering frames throughout, both lanes land on
/// `RefinerMode::Off` (Apple + lexicon), and the heavyweight Whisper
/// initializer counters do not move. The depth (draft volatility, ingest
/// doctrine, sleep/wake, stop-drain, truth-seam adjudication) lives in
/// `asr_session::recorder::tests`; what stays here is the fleet-level witness
/// that the fixture the wave precommitted is the fixture production satisfies.
#[test]
fn fleet_red_cloud_backpressure_degrades_to_apple_only() {
    use crate::asr_session::recorder::{
        FanOutVerdict, Layer1Decision, Layer1DegradeReason, Layer1LaneState,
        OVERFLOW_DEGRADE_LIMIT, RecorderLayer1Lane,
    };
    use crate::asr_session::{
        AsrErrorKind, FakeAsrSessionProvider, RefinerMode, SessionId, SessionInput,
    };

    // Saturating delta rather than a reset: the sibling M0 witness
    // (`fleet_red_apple_prewarm_never_loads_whisper`) owns resets under
    // `#[serial]`; a concurrent reset can only shrink this delta, never fake
    // an init that did not happen.
    let whisper_probe = || {
        crate::stt::whisper::singleton::test_init_calls()
            + crate::stt::whisper::singleton::test_load_calls()
    };
    let whisper_before = whisper_probe();

    let input = SessionInput {
        session_id: SessionId::new("fleet-red-c1").expect("non-blank session id"),
        locale: Some("pl-PL".to_string()),
        sample_rate: 16_000,
    };
    let frame = [0.1f32; 320];

    // Arm 1 — sustained overflow. Every offer must return without surfacing an
    // error to capture, and the lane must degrade instead of blocking.
    let overflow_provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession)
        .failing_pushes(AsrErrorKind::Overflow);
    let mut overflow_lane =
        RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(overflow_provider)), &input);
    let mut capture_continued = true;
    for _ in 0..(OVERFLOW_DEGRADE_LIMIT * 2) {
        capture_continued &= matches!(
            overflow_lane.offer_pcm(&frame),
            FanOutVerdict::Forwarded | FanOutVerdict::DroppedOverflow | FanOutVerdict::Inactive
        );
    }
    let overflow_degraded = matches!(
        overflow_lane.state(),
        Layer1LaneState::Degraded(Layer1DegradeReason::Overflow)
    );

    // Arm 2 — transport disconnect mid-recording. Capture keeps offering after
    // the session dies; the lane absorbs the offers silently.
    let disconnect_provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession)
        .failing_pushes(AsrErrorKind::Transport);
    let mut disconnect_lane =
        RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(disconnect_provider)), &input);
    for _ in 0..4 {
        capture_continued &= matches!(
            disconnect_lane.offer_pcm(&frame),
            FanOutVerdict::Forwarded | FanOutVerdict::DroppedOverflow | FanOutVerdict::Inactive
        );
    }
    let disconnect_degraded = matches!(
        disconnect_lane.state(),
        Layer1LaneState::Degraded(Layer1DegradeReason::Disconnect(AsrErrorKind::Transport))
    );

    // Both recordings finish on canvas + lexicon, and the bounded stop path
    // never propagates the Layer 1 failure.
    let apple_only = overflow_lane.refiner_mode() == RefinerMode::Off
        && disconnect_lane.refiner_mode() == RefinerMode::Off;
    let overflow_outcome = overflow_lane.stop();
    let disconnect_outcome = disconnect_lane.stop();
    assert!(overflow_outcome.finals().is_empty());
    assert!(disconnect_outcome.finals().is_empty());

    let outcome: Result<BackpressureOutcome, MissingContract> = Ok(BackpressureOutcome {
        capture_continued,
        apple_only,
        overflow_degraded,
        disconnect_degraded,
        whisper_init_calls: whisper_probe().saturating_sub(whisper_before),
    });
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
