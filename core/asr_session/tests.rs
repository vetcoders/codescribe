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

/// Restore every test overlay, including on a failed assertion.
struct Layer1TestEnv {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Layer1TestEnv {
    fn new(root: &std::path::Path) -> Self {
        let overlays = [
            ("CODESCRIBE_DATA_DIR", root.as_os_str().to_owned()),
            (
                "CODESCRIBE_ENV_PATH",
                root.join("absent.env").into_os_string(),
            ),
            ("CODESCRIBE_LAYERED_TRANSCRIPTION", "phase1".into()),
            ("STT_TAIL_PROVIDER", "inprocess".into()),
            ("STT_LIVE_ENDPOINT", "wss://gateway.invalid/live".into()),
            ("STT_LIVE_API_KEY", "fixture-live-key".into()),
        ];
        let mut saved = Vec::new();
        for (key, value) in overlays {
            saved.push((key, std::env::var_os(key)));
            // SAFETY: callers hold the suite's serial environment lock.
            unsafe { std::env::set_var(key, value) };
        }
        Self { saved }
    }
}

impl Drop for Layer1TestEnv {
    fn drop(&mut self) {
        for (key, value) in self.saved.iter().rev() {
            // SAFETY: callers hold the suite's serial environment lock.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn production_layer1_decision_follows_resolved_asr_mode() {
    use super::{Layer1Decision, RecorderLayer1Lane};
    use crate::config::{Config, UserSettings};

    let root = tempfile::tempdir().unwrap();
    let _environment = Layer1TestEnv::new(root.path());
    for (mode, consent, expected_reason, armed) in [
        ("cloud", Some("granted"), "cloud_ready", true),
        ("cloud", None, "consent_missing", false),
        (
            "local_power",
            Some("granted"),
            "local_helper_unavailable",
            false,
        ),
    ] {
        let settings = UserSettings {
            asr_mode: Some(mode.into()),
            cloud_consent: consent.map(str::to_owned),
            layered_transcription: Some("phase1".into()),
            stt_live_endpoint: Some("wss://gateway.invalid/live".into()),
            ..Default::default()
        };
        settings.save().unwrap();
        let snapshot = Config::load_runtime_snapshot_without_keychain().unwrap();
        let mut factory_calls = 0;
        let (decision, receipt) =
            super::layer1_decision_with_factory(&snapshot, |snapshot, _authorization| {
                factory_calls += 1;
                assert_eq!(
                    snapshot.values().stt_live_endpoint.as_deref(),
                    Some("wss://gateway.invalid/live")
                );
                assert_eq!(
                    snapshot.values().stt_live_api_key.as_deref(),
                    Some("fixture-live-key")
                );
                Ok(Box::new(FakeAsrSessionProvider::with_script(
                    RefinerMode::CloudSession,
                    vec![partial(1, 1, "live fixture")],
                )))
            });
        assert_eq!(factory_calls, usize::from(armed));
        assert_eq!(matches!(&decision, Layer1Decision::Armed(_)), armed);
        assert_eq!(receipt.reason, expected_reason);
        assert_eq!(
            receipt.consent,
            if consent.is_some() {
                "granted"
            } else {
                "missing"
            }
        );
        assert_eq!(
            receipt.refiner,
            if armed {
                "cloud_session"
            } else if mode == "local_power" {
                "local_tail_patch"
            } else {
                "off"
            }
        );
        let mut lane = RecorderLayer1Lane::open(decision, &fake_input());
        lane.offer_pcm(&[0.1; 160]);
        lane.poll();
        assert_eq!(lane.telemetry().frames_forwarded, u64::from(armed));
        assert_eq!(lane.telemetry().partials_applied, u64::from(armed));

        // The public production entrypoint must share the same policy and
        // construct a dormant real provider, without connecting in this test.
        let (production, production_receipt) = super::layer1_decision(&snapshot);
        assert_eq!(matches!(production, Layer1Decision::Armed(_)), armed);
        assert_eq!(production_receipt, receipt);
    }
}

#[test]
#[serial_test::serial]
fn apple_only_never_arms_a_refiner() {
    use super::Layer1Decision;
    use crate::config::{Config, UserSettings};

    let root = tempfile::tempdir().unwrap();
    let mut environment = Layer1TestEnv::new(root.path());
    for phase in ["", "phase1", "off", "phase2"] {
        environment.set("CODESCRIBE_LAYERED_TRANSCRIPTION", phase);
        for (mode, consent) in [
            ("apple_only", None),
            ("apple_only", Some("granted")),
            ("cloud", None),
            ("cloud", Some("denied")),
        ] {
            UserSettings {
                asr_mode: Some(mode.into()),
                cloud_consent: consent.map(str::to_owned),
                ..Default::default()
            }
            .save()
            .unwrap();
            let snapshot = Config::load_runtime_snapshot_without_keychain().unwrap();
            let (decision, receipt) = super::layer1_decision_with_factory(&snapshot, |_, _| {
                panic!("Apple-only or refused consent reached cloud construction")
            });
            assert!(
                !decision.is_armed(),
                "mode={mode}, consent={consent:?}, phase={phase}"
            );
            assert!(matches!(decision, Layer1Decision::Disarmed));
            assert!(decision.local_tail_patch_disposition().is_none());
            assert_eq!(receipt.refiner, "off");
        }
    }
}

impl Layer1TestEnv {
    fn set(&mut self, key: &'static str, value: &str) {
        self.saved.push((key, std::env::var_os(key)));
        // SAFETY: callers hold the suite's serial environment lock.
        unsafe { std::env::set_var(key, value) };
    }
}

#[test]
#[serial_test::serial]
fn production_layer1_refusals_never_construct_a_cloud_provider() {
    use super::Layer1Decision;
    use crate::config::{Config, UserSettings};

    let root = tempfile::tempdir().unwrap();
    let mut environment = Layer1TestEnv::new(root.path());
    for (consent, phase, reason) in [
        ("denied", "phase1", "consent_denied"),
        ("granted", "off", "layered_off"),
        ("granted", "phase2", "layered_invalid"),
    ] {
        environment.set("CODESCRIBE_LAYERED_TRANSCRIPTION", phase);
        UserSettings {
            asr_mode: Some("cloud".into()),
            cloud_consent: Some(consent.into()),
            ..Default::default()
        }
        .save()
        .unwrap();
        let snapshot = Config::load_runtime_snapshot_without_keychain().unwrap();
        let (decision, receipt) = super::layer1_decision_with_factory(&snapshot, |_, _| {
            panic!("refused selection reached cloud construction")
        });
        assert!(!matches!(decision, Layer1Decision::Armed(_)));
        assert_eq!(receipt.reason, reason);
        assert_eq!(receipt.consent, consent);
    }

    environment.set("CODESCRIBE_LAYERED_TRANSCRIPTION", "phase1");
    let snapshot = Config::load_runtime_snapshot_without_keychain().unwrap();
    let (decision, receipt) =
        super::layer1_decision_with_factory(&snapshot, |_, _| Err("live_connection_invalid"));
    assert!(matches!(decision, Layer1Decision::Disarmed));
    assert_eq!(receipt.reason, "live_connection_invalid");
    assert_eq!(receipt.refiner, "off");

    // A sealed snapshot remains authoritative after process inputs change.
    environment.set("STT_LIVE_ENDPOINT", "not-a-websocket");
    environment.set("STT_LIVE_API_KEY", "");
    let (decision, receipt) = super::layer1_decision(&snapshot);
    assert!(matches!(decision, Layer1Decision::Armed(_)));
    assert_eq!(receipt.reason, "cloud_ready");
}

#[test]
#[serial_test::serial]
fn production_layer1_cloud_forwards_native_pcm_over_real_websocket() {
    use super::{Layer1Decision, RecorderLayer1Lane};
    use crate::config::{Config, UserSettings};
    use std::time::{Duration, Instant};
    use tokio_tungstenite::tungstenite::Message;

    let root = tempfile::tempdir().unwrap();
    let mut environment = Layer1TestEnv::new(root.path());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!(
        // WHY: test-only loopback server on 127.0.0.1 with no TLS and no network egress;
        // WHEN: unit tests only; WHERE: the production lane takes the wss:// live endpoint
        // from the loader snapshot, never this literal (semgrep detect-insecure-websocket).
        "ws://{}/v1/audio/transcribe", // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
        listener.local_addr().unwrap()
    );
    environment.set("STT_LIVE_ENDPOINT", &endpoint);
    UserSettings {
        asr_mode: Some("cloud".into()),
        cloud_consent: Some("granted".into()),
        stt_live_endpoint: Some(endpoint),
        ..Default::default()
    }
    .save()
    .unwrap();
    let snapshot = Config::load_runtime_snapshot_without_keychain().unwrap();
    let (decision, receipt) = super::layer1_decision(&snapshot);
    assert!(matches!(decision, Layer1Decision::Armed(_)));
    assert_eq!(receipt.refiner, "cloud_session");

    let (received_tx, received_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "cloud connection never arrived");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("loopback accept failed: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut socket = tokio_tungstenite::tungstenite::accept(stream).unwrap();
        let Message::Text(start) = socket.read().unwrap() else {
            panic!("missing set frame")
        };
        let start: serde_json::Value = serde_json::from_str(&start).unwrap();
        assert_eq!(start["type"], "set");
        assert_eq!(start["sample_rate"], 88_200);
        let Message::Text(chunk) = socket.read().unwrap() else {
            panic!("missing PCM frame")
        };
        let chunk: serde_json::Value = serde_json::from_str(&chunk).unwrap();
        assert_eq!(chunk["type"], "chunk");
        received_tx.send(()).unwrap();
        socket
            .send(Message::Text(
                r#"{"type":"transcript.partial","text":"loopback fixture"}"#.into(),
            ))
            .unwrap();
        while let Ok(message) = socket.read() {
            if let Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["type"] == "end" {
                    socket
                        .send(Message::Text(
                            r#"{"type":"session.ended","session_id":"session-a"}"#.into(),
                        ))
                        .unwrap();
                    break;
                }
            }
        }
    });
    let mut input = fake_input();
    input.sample_rate = 88_200;
    let mut lane = RecorderLayer1Lane::open(decision, &input);
    assert!(lane.is_live());
    // Native-rate 100 ms frame exceeds the old 16 kHz-only 3200-sample budget.
    lane.offer_pcm(&[0.1; 8_820]);
    received_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while lane.telemetry().partials_applied == 0 && Instant::now() < deadline {
        lane.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(lane.telemetry().frames_forwarded, 1);
    assert_eq!(lane.telemetry().partials_applied, 1);
    assert_eq!(lane.telemetry().provider_errors, 0);
    let outcome = lane.stop();
    assert_eq!(outcome.telemetry().frames_forwarded, 1);
    assert_eq!(outcome.telemetry().partials_applied, 1);
    assert_eq!(outcome.degrade_reason(), None);
    server.join().unwrap();
}
