//! Integrator-authored event-sequence falsifiers. These exercise real reducer,
//! Bus persistence and Stop; they do not claim microphone/model acceptance.
use super::emitter::PresentationEmitter;
use super::transcript_bus::{
    TranscriptBus, TranscriptDelivery, TranscriptMode, TranscriptSession,
    TranscriptSessionEndReason,
};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, ObservationIdentity, ObservationProducer, OccurrenceIdentity,
};
use codescribe_core::pipeline::contracts::{EngineEvent, EventSink, PreviewPin};
use codescribe_core::stt::tail_provider::TailSampleRange;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

struct Take {
    emitter: PresentationEmitter,
    delivery: Arc<Mutex<String>>,
    ledger: Arc<StdMutex<AcousticLedger>>,
    bus_path: std::path::PathBuf,
    bus: Arc<TranscriptBus>,
    _temp: tempfile::TempDir,
}

fn take(apple: bool) -> Take {
    take_for_mode(apple, false)
}

fn take_for_mode(apple: bool, agent: bool) -> Take {
    let temp = tempfile::tempdir().unwrap();
    let bus_path = temp.path().join("direct.jsonl");
    let bus = Arc::new(
        TranscriptBus::open_at(
            TranscriptSession {
                session_id: "translation".into(),
                mode: if agent {
                    TranscriptMode::Agent
                } else {
                    TranscriptMode::Dictation
                },
                has_latched_target: true,
                latched_target_is_self: false,
                audience: agent.then(|| "lena".into()),
                badge_only: agent,
            },
            bus_path.clone(),
            None,
        )
        .unwrap(),
    );
    bus.publish_started();
    let delivery = Arc::new(Mutex::new(String::new()));
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let emitter = PresentationEmitter::new_with_authority(
        Arc::clone(&delivery),
        None,
        None,
        Some(Arc::clone(&bus)),
        Some(Arc::clone(&ledger)),
        None,
    )
    .with_transcription_controls(false, apple);
    emitter.on_capture_opened("translation", 7);
    Take {
        emitter,
        delivery,
        ledger,
        bus_path,
        bus,
        _temp: temp,
    }
}

fn direct(
    producer: ObservationProducer,
    request: u64,
    start: u64,
    end: u64,
    text: &str,
) -> EngineEvent {
    EngineEvent::DirectTranscript {
        observation: ObservationIdentity::new(
            producer,
            request,
            0,
            OccurrenceIdentity::new("translation", 7, start, end),
        ),
        text: text.into(),
        final_chunk: true,
    }
}

fn terminal() -> EngineEvent {
    EngineEvent::DirectTranscriptEnd {
        session_id: "translation".into(),
        capture_epoch: 7,
    }
}

#[tokio::test]
async fn direct_translation_survives_disabled_apple_and_stop_without_forged_ledger() {
    let mut take = take(false);
    let expected = "I can speak Polish and keep the English translation.";
    take.emitter
        .on_event(&direct(ObservationProducer::Whisper, 1, 0, 48000, expected));
    take.emitter.on_event(&direct(
        ObservationProducer::Apple,
        2,
        0,
        48000,
        "Mogę mówić po polsku i zachować angielskie tłumaczenie.",
    ));
    take.emitter.on_event(&EngineEvent::Preview {
        rev: 3,
        text: "późny polski podgląd".into(),
        pin: PreviewPin::from_segments(TailSampleRange {
            session: "translation".into(),
            capture_epoch: 7,
            sample_start: 0,
            sample_end: 48000,
        }),
    });
    let before = take.emitter.visible_canvas_snapshot().unwrap();
    assert_eq!(before.text, expected);
    assert!(before.has_committed_document);
    assert_eq!(before.preview_only_words, 0);
    let frozen = take.emitter.begin_stop_canvas().unwrap();
    take.emitter.on_event(&terminal());
    let shaped = take
        .emitter
        .shape_frozen_canvas_at_stop(frozen)
        .expect("direct Stop is valid");
    assert_eq!(shaped.text, expected);
    take.emitter.finish_stop_canvas();
    take.emitter.finish().await;
    assert_eq!(*take.delivery.lock().await, expected);
    let ledger = take.ledger.lock().unwrap();
    assert_eq!(ledger.qualified_occurrences().count(), 0);
    assert!(ledger.manual_document_revisions().is_empty());
    let persisted = std::fs::read_to_string(&take.bus_path).unwrap();
    assert!(persisted.contains(expected));
    assert!(!persisted.contains("późny polski podgląd"));
}

#[tokio::test]
async fn direct_translation_beats_late_apple_for_the_same_pcm_when_apple_is_enabled() {
    let mut take = take(true);
    take.emitter.on_event(&direct(
        ObservationProducer::Apple,
        1,
        0,
        48000,
        "Mówię po polsku.",
    ));
    take.emitter.on_event(&direct(
        ObservationProducer::Whisper,
        2,
        0,
        48000,
        "I am speaking Polish.",
    ));
    take.emitter.on_event(&direct(
        ObservationProducer::Apple,
        3,
        0,
        48000,
        "Mówię właśnie po polsku.",
    ));
    assert_eq!(
        take.emitter.visible_canvas_snapshot().unwrap().text,
        "I am speaking Polish."
    );
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    assert_eq!(*take.delivery.lock().await, "I am speaking Polish.");
}

#[tokio::test]
async fn direct_five_iwo_uses_five_pcm_ranges_and_request_replay_is_idempotent() {
    let mut take = take(false);
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/p0_b_five_iwo_manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/p0_b_five_iwo.wav");
    let pcm = hound::WavReader::open(fixture)
        .unwrap()
        .samples::<i16>()
        .map(|sample| f64::from(sample.unwrap()) / 32768.0)
        .collect::<Vec<_>>();
    let bursts = manifest["bursts"].as_array().unwrap();
    assert_eq!(bursts.len(), 5);
    for (index, burst) in bursts.iter().enumerate() {
        let start = burst["sample_start"].as_u64().unwrap() as usize;
        let end = burst["sample_end"].as_u64().unwrap() as usize;
        let energy: f64 = pcm[start..end].iter().map(|sample| sample * sample).sum();
        assert!(energy >= manifest["minimum_energy_integral"].as_f64().unwrap());
        let event = direct(
            ObservationProducer::Whisper,
            index as u64 + 1,
            burst["sample_start"].as_u64().unwrap(),
            burst["sample_end"].as_u64().unwrap(),
            "Iwo",
        );
        take.emitter.on_event(&event);
        take.emitter.on_event(&event);
    }
    let text = take.emitter.visible_canvas_snapshot().unwrap().text;
    assert_eq!(
        text.split_whitespace()
            .filter(|word| word.trim_end_matches('.') == "Iwo")
            .count(),
        5
    );
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    assert_eq!(
        take.delivery
            .lock()
            .await
            .split_whitespace()
            .filter(|word| word.trim_end_matches('.') == "Iwo")
            .count(),
        5
    );
    assert_eq!(
        take.ledger.lock().unwrap().qualified_occurrences().count(),
        0
    );
}

#[tokio::test]
async fn direct_publication_rejects_foreign_capture_and_post_terminal_callbacks() {
    let mut take = take(false);
    take.emitter.on_event(&direct(
        ObservationProducer::Whisper,
        1,
        0,
        16000,
        "Keep this.",
    ));
    for (session, epoch) in [("another-take", 7), ("translation", 8)] {
        take.emitter.on_event(&EngineEvent::DirectTranscript {
            observation: ObservationIdentity::new(
                ObservationProducer::Whisper,
                2,
                0,
                OccurrenceIdentity::new(session, epoch, 16000, 32000),
            ),
            text: "Foreign words.".into(),
            final_chunk: true,
        });
    }
    take.emitter.on_event(&terminal());
    take.emitter.on_event(&direct(
        ObservationProducer::Whisper,
        3,
        16000,
        32000,
        "Too late.",
    ));
    take.emitter.finish().await;
    assert_eq!(
        take.emitter.visible_canvas_snapshot().unwrap().text,
        "Keep this."
    );
    assert_eq!(*take.delivery.lock().await, "Keep this.");
}

#[tokio::test]
async fn direct_agent_terminal_holds_words_without_claiming_failed_acoustic_coverage() {
    let mut take = take_for_mode(false, true);
    take.emitter.on_event(&direct(
        ObservationProducer::Whisper,
        1,
        0,
        16000,
        "Keep the English message.",
    ));
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    take.bus.publish_channel_closed(10).unwrap();
    let raw = std::fs::read_to_string(&take.bus_path).unwrap();
    let closing: Vec<serde_json::Value> = raw
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|row| row["status"] == "transcript_sealed")
        .collect();
    assert_eq!(closing.len(), 2, "normal evidence and clean closing twins");
    assert_eq!(closing[0]["hold_until"], closing[1]["hold_until"]);
    assert!(closing[0]["hold_until"].is_string());
    assert!(
        closing.iter().all(|row| row["coverage"] != "refused"),
        "opting out of the ledger is not an acoustic refusal"
    );
    assert!(
        take.bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted
            )
            .is_some()
    );
    assert_eq!(
        take.ledger.lock().unwrap().qualified_occurrences().count(),
        0
    );
}

#[tokio::test]
async fn direct_empty_agent_terminal_settles_without_inventing_a_word() {
    let mut take = take_for_mode(false, true);
    take.emitter.on_event(&EngineEvent::NoSpeech {
        reason: "selected recognizer unavailable".into(),
    });
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    take.bus.publish_channel_closed(10).unwrap();
    assert!(
        take.bus
            .publish_ended(
                TranscriptSessionEndReason::TranscriptionFailed,
                false,
                TranscriptDelivery::Unattempted
            )
            .is_some(),
        "no-source terminal must release lifecycle"
    );
    assert!(take.delivery.lock().await.is_empty());
    assert_eq!(
        take.ledger.lock().unwrap().qualified_occurrences().count(),
        0
    );
}

fn apple_hypothesis(
    request: u64,
    generation: u64,
    end: u64,
    text: &str,
    final_chunk: bool,
) -> EngineEvent {
    EngineEvent::DirectTranscript {
        observation: ObservationIdentity::new(
            ObservationProducer::Apple,
            request,
            generation,
            OccurrenceIdentity::new("translation", 7, 0, end),
        ),
        text: text.into(),
        final_chunk,
    }
}

#[tokio::test]
async fn direct_growing_apple_partials_replace_one_hypothesis_before_final() {
    let mut take = take(true);
    take.emitter
        .on_event(&apple_hypothesis(1, 1, 100, "Old partial", false));
    take.emitter
        .on_event(&apple_hypothesis(1, 2, 200, "Longer partial", false));
    take.emitter
        .on_event(&apple_hypothesis(1, 3, 180, "Correct final.", true));
    assert_eq!(
        take.emitter.visible_canvas_snapshot().unwrap().text,
        "Correct final."
    );
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    assert_eq!(*take.delivery.lock().await, "Correct final.");
}

#[tokio::test]
async fn direct_unchanged_newer_hypothesis_advances_request_freshness() {
    let mut take = take(true);
    take.emitter
        .on_event(&apple_hypothesis(1, 1, 200, "Current", false));
    take.emitter
        .on_event(&apple_hypothesis(1, 3, 200, "Current", false));
    take.emitter
        .on_event(&apple_hypothesis(1, 2, 200, "Stale replacement", false));
    assert_eq!(
        take.emitter.visible_canvas_snapshot().unwrap().text,
        "Current"
    );
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    assert_eq!(*take.delivery.lock().await, "Current");
}

#[tokio::test]
async fn direct_late_apple_with_different_overlapping_range_cannot_append_to_english() {
    let mut take = take(true);
    take.emitter.on_event(&direct(
        ObservationProducer::Whisper,
        1,
        0,
        200,
        "English survives.",
    ));
    take.emitter
        .on_event(&apple_hypothesis(2, 1, 210, "Polska nadpiska", true));
    assert_eq!(
        take.emitter.visible_canvas_snapshot().unwrap().text,
        "English survives."
    );
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    assert_eq!(*take.delivery.lock().await, "English survives.");
}

#[tokio::test]
async fn direct_empty_end_closes_admission_before_any_word_arrives() {
    let mut take = take(false);
    take.emitter.on_event(&terminal());
    take.emitter.on_event(&direct(
        ObservationProducer::Whisper,
        1,
        0,
        16000,
        "Too late.",
    ));
    take.emitter.finish().await;
    assert!(take.delivery.lock().await.is_empty());
    assert!(
        take.emitter
            .visible_canvas_snapshot()
            .map(|s| s.text)
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn direct_disabled_apple_cannot_seed_an_empty_document() {
    let mut take = take(false);
    take.emitter
        .on_event(&apple_hypothesis(1, 1, 200, "Disabled source", false));
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    assert!(take.delivery.lock().await.is_empty());
}

#[tokio::test]
async fn direct_duplicate_observation_identity_cannot_replace_its_original_text() {
    let mut take = take(true);
    take.emitter
        .on_event(&apple_hypothesis(1, 1, 200, "Original", false));
    take.emitter
        .on_event(&apple_hypothesis(1, 1, 200, "Replayed rewrite", false));
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    assert_eq!(*take.delivery.lock().await, "Original");
}

#[tokio::test]
async fn direct_translation_retires_apple_preview_for_the_same_audio() {
    let mut take = take(true);
    take.emitter.on_event(&EngineEvent::Preview {
        rev: 1,
        text: "Polski podgląd".into(),
        pin: PreviewPin::from_segments(TailSampleRange {
            session: "translation".into(),
            capture_epoch: 7,
            sample_start: 0,
            sample_end: 48000,
        }),
    });
    take.emitter.on_event(&direct(
        ObservationProducer::Whisper,
        1,
        0,
        48000,
        "English translation.",
    ));
    take.emitter.on_event(&terminal());
    take.emitter.finish().await;
    assert_eq!(
        take.emitter.visible_canvas_snapshot().unwrap().text,
        "English translation."
    );
    assert_eq!(*take.delivery.lock().await, "English translation.");
}
