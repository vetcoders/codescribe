use super::*;
use codescribe_core::pipeline::acoustic_ledger::{
    ObservationIdentity, ObservationProducer, OccurrenceIdentity,
};
use codescribe_core::pipeline::contracts::EngineEvent;

use super::super::speech_clock::measured_clock_edge;

#[tokio::test]
async fn channel_factory_uses_all_four_frozen_track_choices() {
    for ledger_on in [false, true] {
        for apple_on in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let session = "agent-channel-2-controls";
            let path = dir.path().join("channel.jsonl");
            let bus = Arc::new(
                TranscriptBus::open_at(
                    TranscriptSession {
                        session_id: session.into(),
                        mode: TranscriptMode::Agent,
                        has_latched_target: false,
                        latched_target_is_self: false,
                        audience: Some("lena".into()),
                        badge_only: true,
                    },
                    path.clone(),
                    None,
                )
                .unwrap(),
            );
            bus.publish_started();
            let mut inputs = codescribe_core::config::CapturedRuntimeInputs::defaults_at(
                dir.path().to_owned(),
                1,
            );
            inputs.user_settings.seal_ledger_enabled = Some(ledger_on);
            inputs.user_settings.apple_live_enabled = Some(apple_on);
            let settings = Config::runtime_snapshot_from_captured(inputs);
            let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
            let mut emitter = channel_presentation_emitter(bus.clone(), ledger.clone(), &settings);
            emitter.on_capture_opened(session, 7);
            let event = |producer, request, text: &str| EngineEvent::DirectTranscript {
                observation: ObservationIdentity::new(
                    producer,
                    request,
                    0,
                    OccurrenceIdentity::new(session, 7, 0, 16000),
                ),
                text: text.into(),
                final_chunk: true,
            };
            emitter.on_event(&event(ObservationProducer::Apple, 1, "Polski podgląd."));
            let seeded = emitter.visible_canvas_snapshot().unwrap().text;
            assert_eq!(
                seeded,
                if !ledger_on && apple_on {
                    "Polski podgląd."
                } else {
                    ""
                }
            );
            emitter.on_event(&event(ObservationProducer::Whisper, 2, "English survives."));
            emitter.on_event(&event(
                ObservationProducer::Apple,
                3,
                "Spóźniony polski tekst.",
            ));
            emitter.on_event(&EngineEvent::DirectTranscriptEnd {
                session_id: session.into(),
                capture_epoch: 7,
            });
            emitter.finish().await;
            assert_eq!(
                emitter.visible_canvas_snapshot().unwrap().text,
                if ledger_on { "" } else { "English survives." },
                "ledger={ledger_on} apple={apple_on}"
            );
            if !ledger_on {
                bus.publish_channel_closed(10, None).unwrap();
                let rows = std::fs::read_to_string(path).unwrap();
                assert!(rows.contains("English survives."));
                assert!(rows.contains("\"coverage\":\"direct\""));
            }
            assert_eq!(ledger.lock().unwrap().qualified_occurrences().count(), 0);
        }
    }
}
pub(super) fn measured_speech_end(open: &OpenAgentChannel, at: SystemTime) {
    let mut clock = open.speech_clock.lock().unwrap();
    let (seq, sample) = clock
        .speech
        .last_edge
        .as_ref()
        .map_or((1, 0), |edge| (edge.sequence + 1, edge.sample + 1));
    clock
        .speech
        .observe(&measured_clock_edge(seq, sample, true), at);
    clock
        .speech
        .observe(&measured_clock_edge(seq + 1, sample + 16_000, false), at);
}

/// The playback guard reads the phase file: a fresh session reports quiet
/// since it opened, speech holds it, a broken edge chain is never quiet,
/// and closing the channel removes the file.
#[test]
fn capture_clock_publishes_its_phase_for_the_playback_guard() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime").join("channel-speech-3.json");
    let read =
        || -> serde_json::Value { serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap() };
    let opened = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let t = opened + Duration::from_secs(2);
    let mut clock = ChannelSpeechClock::default();
    clock.publish_now(t);
    assert!(
        !path.exists(),
        "nothing is published before the session opens"
    );
    clock.publish_to(path.clone(), 3, "agent-channel-3-x".into(), opened);
    let row = read();
    assert_eq!(row["schema"], CHANNEL_SPEECH_NOW_SCHEMA);
    assert_eq!(row["channel"], "3");
    assert_eq!(row["session_id"], "agent-channel-3-x");
    assert_eq!(row["phase"], "fresh");
    assert_eq!(row["since_ms"], 1_000_000);
    assert_eq!(row["pid"], std::process::id());
    clock.speech.observe(&measured_clock_edge(1, 0, true), t);
    clock.publish_now(t);
    assert_eq!(read()["phase"], "speaking");
    clock
        .speech
        .observe(&measured_clock_edge(2, 16_000, false), t);
    clock.publish_now(t + Duration::from_secs(9));
    let row = read();
    assert_eq!(row["phase"], "quiet");
    assert_eq!(
        row["since_ms"], 1_002_000,
        "quiet dates from the measured end"
    );
    clock
        .speech
        .observe(&measured_clock_edge(3, 32_000, false), t);
    clock.publish_now(t);
    assert_eq!(read()["phase"], "unmeasured");
    clock.withdraw_now();
    assert!(!path.exists());
    clock.publish_now(t);
    assert!(!path.exists(), "a closed session never writes again");
}

#[tokio::test]
async fn capture_clock_real_emitter_forwards_only_the_opened_captures_sideband() {
    use crate::presentation::emitter::PresentationEmitter;
    use codescribe_core::pipeline::contracts::EngineEvent;
    let clock = Arc::new(StdMutex::new(ChannelSpeechClock::default()));
    let observed = Arc::clone(&clock);
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let mut emitter = PresentationEmitter::new_with_authority(
        Arc::new(TokioMutex::new(String::new())),
        None,
        None,
        None,
        None,
        None,
    )
    .with_transcription_controls(false, false)
    .with_speech_activity_observer(Arc::new(move |edge| {
        observed.lock().unwrap().speech.observe(edge, t)
    }));
    emitter.on_capture_opened("clock-owner", 1);
    let mut foreign = measured_clock_edge(1, 0, true);
    foreign.range.session = "foreign".into();
    emitter.on_event(&EngineEvent::SidebandEvidence { evidence: foreign });
    let mut foreign_epoch = measured_clock_edge(1, 0, true);
    foreign_epoch.range.capture_epoch = 2;
    emitter.on_event(&EngineEvent::SidebandEvidence {
        evidence: foreign_epoch,
    });
    emitter.on_event(&EngineEvent::SidebandEvidence {
        evidence: measured_clock_edge(2, 16_000, false),
    });
    assert_eq!(
        clock
            .lock()
            .unwrap()
            .speech
            .deadline(Some(Duration::from_secs(5))),
        None,
        "a foreign start cannot pair with this capture's end"
    );
    emitter.on_event(&EngineEvent::SidebandEvidence {
        evidence: measured_clock_edge(3, 32_000, true),
    });
    assert!(
        !clock
            .lock()
            .unwrap()
            .speech
            .silence_due(t + Duration::from_secs(20), Some(Duration::from_secs(5)))
    );
    emitter.on_event(&EngineEvent::SidebandEvidence {
        evidence: measured_clock_edge(4, 48_000, false),
    });
    assert_eq!(
        clock
            .lock()
            .unwrap()
            .speech
            .deadline(Some(Duration::from_secs(5))),
        Some(t + Duration::from_secs(5))
    );
    emitter.finish().await;
}

#[test]
fn founder_channel_10_constants_and_bus_path() {
    let root = Path::new("/tmp/agent-bridge");
    assert_eq!(FOUNDER_CHANNEL, "10");
    assert_eq!(FOUNDER_AUDIENCE, "founder");
    assert_eq!(
        founder_bus_path_at(root),
        PathBuf::from("/tmp/agent-bridge/buses/channel-10.jsonl")
    );
}
