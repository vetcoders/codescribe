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
//! `crate::asr_session`; deleting a `missing(...)` call without a seam behind it
//! would be the opposite of that.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingContract {
    BoundedAppleOnlyDegradation,
    KillableLocalHelper,
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

/// PROMOTED (C2, `c2-cloud-mode-consent`): this probe no longer stops at a
/// `MissingContract` boundary. The seam it named exists — the consent gate in
/// `asr_session::consent` plus the mode/consent resolver in
/// `config::cloud_asr` — and the same rejection now comes from the production
/// factory. The depth (wire parsing, upgrade preservation, settings
/// round-trip, gateway mint validation) lives in those modules' tests; what
/// stays here is the fleet-level witness that cloud session construction
/// without explicit audio-egress consent is refused with a typed error, and
/// that the refusal never reaches for local weights.
#[test]
fn fleet_red_cloud_requires_explicit_consent() {
    use crate::asr_session::consent::{CloudSessionError, authorize_cloud_egress, refiner_for};
    use crate::asr_session::provider::RefinerMode;
    use crate::config::cloud_asr::{
        AsrProductMode, AudioEgressConsent, ModeDerivation, resolve_asr_product_mode,
    };

    // Session construction without explicit consent: typed rejection.
    for withheld in [AudioEgressConsent::Unanswered, AudioEgressConsent::Denied] {
        assert_eq!(
            authorize_cloud_egress(&withheld).err(),
            Some(CloudSessionError::ConsentRequired),
            "session construction without explicit audio-egress consent must be rejected by the production factory"
        );
    }

    // A fresh install that persisted `cloud` but never answered the consent
    // question resolves to Apple-only and arms no Layer 1 provider — and in
    // particular never the local helper.
    let unconsented = resolve_asr_product_mode(Some("cloud"), None, None);
    assert_eq!(unconsented.mode, AsrProductMode::AppleOnly);
    assert_eq!(
        unconsented.derivation,
        ModeDerivation::ConsentMissingFallback
    );
    assert_eq!(refiner_for(&unconsented), RefinerMode::Off);

    // With the explicit grant recorded, the same request is authorized and
    // arms the cloud session.
    let consented = resolve_asr_product_mode(Some("cloud"), Some("granted"), None);
    assert!(authorize_cloud_egress(&consented.consent).is_ok());
    assert_eq!(refiner_for(&consented), RefinerMode::CloudSession);
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
