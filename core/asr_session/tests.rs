//! Contract tests for the neutral Layer 1 session seam.
//!
//! These are the production witnesses behind the fleet-level RED probe in
//! `stt::fleet_red_contracts`: ordering, duplicate-final idempotence, bounded
//! ranges, payload-free errors, and the canvas/refiner split.

use super::events::{
    AsrErrorKind, AsrSessionEvent, AudioRange, ErrorEvent, SessionId, TranscriptEvent, UsageEvent,
};
use super::fake::FakeAsrSessionProvider;
use super::ingest::{IngestVerdict, SessionIngest};
use super::provider::{
    AsrSessionProvider, CanvasEngine, LayerSelection, RefinerMode, SessionInput,
};

/// Session id used across the ordering tests.
fn session() -> SessionId {
    SessionId::new("session-a").expect("non-blank session id")
}

/// Partial hypothesis for `utterance` at `sequence`.
fn partial(utterance: u64, sequence: u64, text: &str) -> AsrSessionEvent {
    AsrSessionEvent::Partial(TranscriptEvent {
        session_id: session(),
        utterance_id: utterance,
        sequence_id: sequence,
        text: text.to_string(),
        range: None,
    })
}

/// Sealing final for `utterance` at `sequence`.
fn final_event(utterance: u64, sequence: u64, text: &str) -> AsrSessionEvent {
    AsrSessionEvent::Final(TranscriptEvent {
        session_id: session(),
        utterance_id: utterance,
        sequence_id: sequence,
        text: text.to_string(),
        range: None,
    })
}

/// Typed failure for `utterance` at `sequence`.
fn error_event(utterance: u64, sequence: u64, kind: AsrErrorKind) -> AsrSessionEvent {
    AsrSessionEvent::Error(ErrorEvent {
        session_id: session(),
        utterance_id: utterance,
        sequence_id: sequence,
        kind,
    })
}

// ═══════════════════════════════════════════════════════════
// Ordering and idempotence
// ═══════════════════════════════════════════════════════════

/// THE ORDERING MATRIX: a live provider replays. Only the sequence orders the
/// stream, a re-sent final changes nothing, and a stale final never lands.
#[test]
fn ingest_orders_by_sequence_and_absorbs_duplicate_finals() {
    let mut ingest = SessionIngest::new(session());

    let verdicts: Vec<IngestVerdict> = vec![
        partial(7, 1, "pacjent ma"),
        final_event(7, 2, "pacjent ma goraczke"),
        final_event(7, 2, "pacjent ma goraczke"), // reconnect resend
        final_event(7, 1, "pacjent"),             // stale, arrives late
        error_event(7, 3, AsrErrorKind::Transport),
    ]
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
        ]
    );
    assert_eq!(
        ingest.accepted(),
        [
            partial(7, 1, "pacjent ma"),
            final_event(7, 2, "pacjent ma goraczke"),
            error_event(7, 3, AsrErrorKind::Transport),
        ]
    );
    assert_eq!(ingest.duplicate_count(), 1);
    assert_eq!(ingest.out_of_order_count(), 1);
    assert_eq!(ingest.last_sequence(), Some(3));

    // The seal still holds the text the accepted final carried — the stale
    // final did not rewrite it.
    let sealed = ingest.sealed_final(7).expect("utterance 7 is sealed");
    assert_eq!(sealed.text, "pacjent ma goraczke");
}

/// A resend that arrives *after* newer events is still the same commitment, so
/// it is idempotent rather than "out of order". This is the reconnect case the
/// sequence check alone would misclassify.
#[test]
fn duplicate_final_is_idempotent_even_after_newer_events() {
    let mut ingest = SessionIngest::new(session());
    assert!(ingest.ingest(final_event(1, 10, "raz dwa")).is_accepted());
    assert!(ingest.ingest(partial(2, 11, "trzy")).is_accepted());

    assert_eq!(
        ingest.ingest(final_event(1, 10, "raz dwa")),
        IngestVerdict::DuplicateIdempotent
    );
    assert_eq!(ingest.accepted().len(), 2);
    assert_eq!(ingest.duplicate_count(), 1);
    assert_eq!(ingest.out_of_order_count(), 0);
}

/// A final is a commitment. A later partial for that utterance — even with a
/// perfectly monotonic sequence — must not reopen it.
#[test]
fn sealed_utterance_refuses_later_partials_and_conflicting_finals() {
    let mut ingest = SessionIngest::new(session());
    assert!(
        ingest
            .ingest(final_event(3, 5, "badanie krwi wykazalo"))
            .is_accepted()
    );

    assert_eq!(
        ingest.ingest(partial(3, 6, "badanie krwi")),
        IngestVerdict::RejectedSealedUtterance
    );
    assert_eq!(
        ingest.ingest(final_event(3, 7, "zupelnie inny tekst")),
        IngestVerdict::RejectedSealedUtterance
    );
    assert_eq!(ingest.sealed_rejection_count(), 2);
    assert_eq!(
        ingest.sealed_final(3).map(|event| event.text.as_str()),
        Some("badanie krwi wykazalo")
    );

    // A different utterance is untouched by the seal.
    assert!(
        ingest
            .ingest(partial(4, 8, "kolejna wypowiedz"))
            .is_accepted()
    );
}

/// Diagnostics are not text: an error for a sealed utterance still lands, so a
/// provider can report a failure after it has already committed a final.
#[test]
fn sealed_utterance_still_accepts_diagnostics() {
    let mut ingest = SessionIngest::new(session());
    assert!(ingest.ingest(final_event(2, 4, "gotowe")).is_accepted());
    assert!(
        ingest
            .ingest(error_event(2, 5, AsrErrorKind::Transport))
            .is_accepted()
    );
    assert_eq!(ingest.sealed_rejection_count(), 0);
}

/// A reconnect that resumes the wrong stream is caught at the ledger edge.
#[test]
fn foreign_session_events_are_refused() {
    let mut ingest = SessionIngest::new(session());
    let foreign = SessionId::new("session-b").expect("non-blank session id");
    let event = AsrSessionEvent::Final(TranscriptEvent {
        session_id: foreign,
        utterance_id: 1,
        sequence_id: 1,
        text: "z innej sesji".to_string(),
        range: None,
    });

    assert_eq!(ingest.ingest(event), IngestVerdict::RejectedForeignSession);
    assert!(ingest.accepted().is_empty());
    assert_eq!(ingest.foreign_rejection_count(), 1);
    assert_eq!(ingest.last_sequence(), None);
}

/// A blank session id would make every session compare equal and silently
/// disable the foreign-session guard.
#[test]
fn blank_session_ids_are_refused() {
    assert!(SessionId::new("").is_none());
    assert!(SessionId::new("   \n").is_none());
    assert_eq!(
        SessionId::new("s-1").map(|id| id.as_str().to_string()),
        Some("s-1".to_string())
    );
}

// ═══════════════════════════════════════════════════════════
// Bounded audio ranges
// ═══════════════════════════════════════════════════════════

/// The optional range is bounded on every axis that could turn a corrupt
/// timestamp into a plausible-looking window.
#[test]
fn audio_range_rejects_unusable_spans() {
    let ok = AudioRange::new(1.0, 2.5).expect("valid span");
    assert_eq!(ok.start_secs(), 1.0);
    assert_eq!(ok.end_secs(), 2.5);
    assert_eq!(ok.duration_secs(), 1.5);

    assert!(AudioRange::new(f32::NAN, 1.0).is_none());
    assert!(AudioRange::new(0.0, f32::INFINITY).is_none());
    assert!(AudioRange::new(-0.5, 1.0).is_none());
    assert!(AudioRange::new(2.0, 2.0).is_none(), "empty span");
    assert!(AudioRange::new(3.0, 1.0).is_none(), "inverted span");
    assert!(
        AudioRange::new(0.0, AudioRange::MAX_SPAN_SECS + 0.1).is_none(),
        "a span wider than retained PCM describes audio nothing can re-read"
    );
    assert!(AudioRange::new(0.0, AudioRange::MAX_SPAN_SECS).is_some());
}

/// The ceiling is the live PCM ring's retention, not an independent number that
/// can drift away from it.
#[test]
fn audio_range_ceiling_tracks_live_pcm_retention() {
    assert_eq!(
        AudioRange::MAX_SPAN_SECS,
        crate::pipeline::streaming::live_audio_buffer::DEFAULT_RETENTION_SECS
    );
}

// ═══════════════════════════════════════════════════════════
// Typed errors and usage
// ═══════════════════════════════════════════════════════════

/// Errors carry a kind and nothing else, so no transcript fragment, audio path,
/// or credential can ride one into a log line.
#[test]
fn errors_are_typed_with_no_free_form_payload() {
    let kinds = [
        (AsrErrorKind::Transport, "transport", true),
        (AsrErrorKind::Auth, "auth", false),
        (AsrErrorKind::RateLimited, "rate_limited", true),
        (AsrErrorKind::Quota, "quota", false),
        (AsrErrorKind::Overflow, "overflow", true),
        (AsrErrorKind::Unsupported, "unsupported", false),
        (AsrErrorKind::Protocol, "protocol", false),
        (AsrErrorKind::Cancelled, "cancelled", false),
    ];
    for (kind, token, retryable) in kinds {
        assert_eq!(kind.as_token(), token);
        assert_eq!(format!("{kind}"), token);
        assert_eq!(kind.is_retryable(), retryable, "{token}");
    }
}

/// Usage is accounting, not content.
#[test]
fn usage_events_carry_accounting_only() {
    let usage = UsageEvent {
        session_id: session(),
        utterance_id: 0,
        sequence_id: 9,
        audio_secs: 12.5,
        billable_units: Some(13),
    };
    let event = AsrSessionEvent::Usage(usage);
    assert_eq!(event.as_token(), "usage");
    assert!(!event.is_transcript());
    assert!(!event.is_final());
    assert_eq!(event.sequence_id(), 9);
}

/// Finality is a variant, so every consumer has to decide about it explicitly.
#[test]
fn finality_is_a_variant_not_a_flag() {
    assert!(partial(1, 1, "x").is_transcript());
    assert!(!partial(1, 1, "x").is_final());
    assert!(final_event(1, 2, "x").is_final());
    assert_eq!(partial(1, 1, "x").as_token(), "partial");
    assert_eq!(final_event(1, 2, "x").as_token(), "final");
    assert_eq!(
        error_event(1, 3, AsrErrorKind::Auth).as_token(),
        "error",
        "an error is never mistaken for text"
    );
}

// ═══════════════════════════════════════════════════════════
// Canvas selection versus refiner mode
// ═══════════════════════════════════════════════════════════

/// The two axes are independent: choosing a refiner never moves the canvas.
#[test]
fn refiner_mode_never_moves_the_canvas() {
    for canvas in [CanvasEngine::AppleSpeech, CanvasEngine::LocalWhisper] {
        for refiner in [
            RefinerMode::Off,
            RefinerMode::CloudSession,
            RefinerMode::LocalHelper,
        ] {
            let selection = LayerSelection::new(canvas, refiner);
            assert_eq!(selection.canvas(), canvas, "{refiner:?} moved the canvas");
            assert_eq!(selection.refiner(), refiner);

            // Layer 1 failing is a missing improvement, never a redraw.
            let degraded = selection.degraded();
            assert_eq!(degraded.canvas(), canvas);
            assert_eq!(degraded.refiner(), RefinerMode::Off);
        }
    }
}

/// `Off` is the shipping product, and it is the default.
#[test]
fn refiner_mode_defaults_to_off_and_classifies_audio_egress() {
    assert_eq!(RefinerMode::default(), RefinerMode::Off);
    assert!(!RefinerMode::Off.sends_audio_off_device());
    assert!(!RefinerMode::LocalHelper.sends_audio_off_device());
    assert!(RefinerMode::CloudSession.sends_audio_off_device());
    assert_eq!(CanvasEngine::AppleSpeech.as_token(), "apple_speech");
    assert_eq!(RefinerMode::CloudSession.as_token(), "cloud_session");
}

// ═══════════════════════════════════════════════════════════
// The fake provider
// ═══════════════════════════════════════════════════════════

/// Session parameters for the fake.
fn fake_input() -> SessionInput {
    SessionInput {
        session_id: session(),
        locale: Some("pl-PL".to_string()),
        sample_rate: 16_000,
    }
}

/// Lifecycle faults degrade into typed errors — a live session must never
/// panic the recording.
#[test]
fn fake_provider_reports_lifecycle_faults_as_protocol_errors() {
    let mut provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession);
    assert_eq!(provider.mode(), RefinerMode::CloudSession);

    assert_eq!(provider.push_audio(&[0.0; 8]), Err(AsrErrorKind::Protocol));
    assert_eq!(provider.close(), Err(AsrErrorKind::Protocol));

    provider.open(&fake_input()).expect("first open succeeds");
    assert_eq!(provider.open(&fake_input()), Err(AsrErrorKind::Protocol));

    provider.close().expect("close after open succeeds");
    assert_eq!(provider.push_audio(&[0.0; 8]), Err(AsrErrorKind::Protocol));
}

/// The fake produces a whole session shape — scripted events in order, then a
/// trailing usage record whose sequence stays monotonic.
#[test]
fn fake_provider_emits_a_monotonic_session() {
    let script = vec![
        partial(1, 1, "pacjent"),
        partial(1, 2, "pacjent ma"),
        final_event(1, 3, "pacjent ma goraczke"),
    ];
    let mut provider = FakeAsrSessionProvider::with_script(RefinerMode::CloudSession, script);
    provider.open(&fake_input()).expect("open");

    assert!(provider.drain().is_empty(), "no audio pushed yet");
    provider.push_audio(&[0.0; 16_000]).expect("push");
    let first = provider.drain();
    assert_eq!(first, vec![partial(1, 1, "pacjent")]);

    provider.push_audio(&[0.0; 8_000]).expect("push");
    assert_eq!(provider.pushed_secs(), 1.5);
    let second = provider.drain();
    assert_eq!(second, vec![partial(1, 2, "pacjent ma")]);

    provider.close().expect("close");
    assert!(provider.script_drained());
    let tail = provider.drain();
    assert_eq!(tail.len(), 2, "trailing final plus usage");
    assert_eq!(tail[0], final_event(1, 3, "pacjent ma goraczke"));
    match &tail[1] {
        AsrSessionEvent::Usage(usage) => {
            assert_eq!(usage.sequence_id, 4, "usage stays monotonic");
            assert_eq!(usage.audio_secs, 1.5);
            assert_eq!(usage.billable_units, None);
        }
        other => panic!("expected a usage record, got {other:?}"),
    }
}

/// A failing transport surfaces its typed kind on every push, and the product
/// answer is the degraded selection — canvas plus lexicon, same canvas.
#[test]
fn fake_provider_push_failure_degrades_to_canvas_only() {
    let mut provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession)
        .failing_pushes(AsrErrorKind::Transport);
    provider.open(&fake_input()).expect("open");

    let error = provider
        .push_audio(&[0.0; 128])
        .expect_err("push must fail");
    assert_eq!(error, AsrErrorKind::Transport);
    assert!(error.is_retryable());
    assert!(provider.drain().is_empty());

    let selection = LayerSelection::new(CanvasEngine::AppleSpeech, RefinerMode::CloudSession);
    let degraded = selection.degraded();
    assert_eq!(degraded.canvas(), CanvasEngine::AppleSpeech);
    assert_eq!(degraded.refiner(), RefinerMode::Off);
}

/// End to end: what the provider emits is what the ledger accepts, and a
/// replayed tail changes nothing.
#[test]
fn fake_provider_stream_survives_a_replayed_tail() {
    let script = vec![
        partial(1, 1, "raz"),
        final_event(1, 2, "raz dwa"),
        final_event(1, 2, "raz dwa"), // the provider re-sends its final
        partial(2, 3, "trzy"),
        final_event(2, 4, "trzy cztery"),
    ];
    let mut provider = FakeAsrSessionProvider::with_script(RefinerMode::LocalHelper, script);
    provider.open(&fake_input()).expect("open");
    provider.close().expect("close flushes the whole script");

    let mut ingest = SessionIngest::new(session());
    let verdicts: Vec<IngestVerdict> = provider
        .drain()
        .into_iter()
        .map(|event| ingest.ingest(event))
        .collect();

    assert_eq!(
        verdicts,
        vec![
            IngestVerdict::Accepted,
            IngestVerdict::Accepted,
            IngestVerdict::DuplicateIdempotent,
            IngestVerdict::Accepted,
            IngestVerdict::Accepted,
            IngestVerdict::Accepted, // the closing usage record
        ]
    );
    assert_eq!(ingest.duplicate_count(), 1);
    assert_eq!(ingest.out_of_order_count(), 0);
    assert_eq!(
        ingest.sealed_final(1).map(|event| event.text.as_str()),
        Some("raz dwa")
    );
    assert_eq!(
        ingest.sealed_final(2).map(|event| event.text.as_str()),
        Some("trzy cztery")
    );
}
