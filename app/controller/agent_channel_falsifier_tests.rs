//! Independent shared-recognizer falsifiers. Synthetic labels, real routing,
//! registry, archives, reducer authentication and durable bus publication.
use super::*;
use crate::presentation::emitter::UserRevisionIntent;
use codescribe_core::audio::channel_demux::{ChannelDemux, DetachSettlement};
use codescribe_core::audio::streaming_recorder::{ChannelSessionSpec, StreamingRecorder};
use codescribe_core::config::{CapturedRuntimeInputs, RuntimeSettingsSnapshot};
use codescribe_core::llm::ai_formatting::{AiFormatResult, AiFormatStatus};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticEvidence, DocumentRevisionProvenance, EnergyCalibration, ObservationIdentity,
    ObservationProducer, OccurrenceIdentity,
};
use codescribe_core::pipeline::contracts::EngineEvent;
use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use std::task::Poll;
use tokio::sync::{mpsc, oneshot};

type FormatterReplies = HashMap<String, oneshot::Receiver<AiFormatResult>>;
static FORMATTERS: LazyLock<StdMutex<FormatterReplies>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Only the provider future is injectable. The production timeout, reducer
/// admission, fallback selection and delivery all still run unchanged.
pub(super) async fn format_for_test(
    session: &str,
    production: impl Future<Output = AiFormatResult>,
) -> AiFormatResult {
    let injected = FORMATTERS.lock().unwrap().remove(session);
    match injected {
        Some(reply) => reply
            .await
            .expect("formatter sender retained until outcome"),
        None => production.await,
    }
}

fn formatter(session: &str) -> oneshot::Sender<AiFormatResult> {
    let (sender, receiver) = oneshot::channel();
    assert!(
        FORMATTERS
            .lock()
            .unwrap()
            .insert(session.into(), receiver)
            .is_none()
    );
    sender
}

fn settings(root: &Path) -> Arc<RuntimeSettingsSnapshot> {
    let mut inputs = CapturedRuntimeInputs::defaults_at(root.to_owned(), 7);
    inputs.user_settings.seal_ledger_enabled = Some(true);
    inputs.user_settings.formatting_level = Some("smart".into());
    Arc::new(Config::runtime_snapshot_from_captured(inputs))
}

struct Segment {
    source: Arc<ChannelSegmentSource>,
    parts: ChannelSegmentParts,
    clock: Arc<StdMutex<ChannelSpeechClock>>,
    paints: Arc<StdMutex<Vec<String>>>,
}

fn segment(
    root: &Path,
    doc: &str,
    digit: u8,
    capture: &str,
    ledger: Arc<StdMutex<AcousticLedger>>,
    seat: DeliverySeat,
) -> Segment {
    let source = Arc::new(ChannelSegmentSource {
        label: doc.into(),
        audience: format!("seat-{digit}"),
        bus: root.join(format!("{doc}.jsonl")),
        broadcast_buses: Vec::new(),
        recipients: Vec::new(),
        runtime_settings: settings(root),
        language: None,
        cursor_token: 0,
        generation: Arc::new(AtomicU64::new(0)),
    });
    let clock = Arc::new(StdMutex::new(ChannelSpeechClock::default()));
    let mut parts = open_channel_segment(
        &source,
        doc.into(),
        ChannelSegmentCapture {
            capture_session_id: capture.into(),
            ledger,
            seat,
        },
        &clock,
        digit,
    );
    // Replace native preview callbacks with an in-process HUD observer.
    let paints = Arc::new(StdMutex::new(Vec::new()));
    let observed = paints.clone();
    let last = parts.last_text.clone();
    let emitter = Arc::try_unwrap(parts.emitter)
        .ok()
        .expect("sole emitter owner")
        .with_apple_preview_observer(Arc::new(|_| {}))
        .with_cursor_observer(Arc::new(move |projection| {
            observed.lock().unwrap().push(projection.text.clone());
            *last.lock().unwrap() = projection.text.clone();
        }));
    parts.emitter = Arc::new(emitter);
    Segment {
        source,
        parts,
        clock,
        paints,
    }
}

fn open(
    segment: &Segment,
    p: ChannelParticipation,
    ledger: Arc<StdMutex<AcousticLedger>>,
) -> OpenAgentChannel {
    OpenAgentChannel {
        subscriber: p.id,
        audience: segment.source.audience.clone(),
        silence_sec: 1.0,
        provider: Some("codex".into()),
        provider_session_id: Some(segment.source.label.clone()),
        opened_at: SystemTime::UNIX_EPOCH,
        speech_clock: segment.clock.clone(),
        session_id: Some(segment.source.label.clone()),
        channel_session_id: Some(segment.source.label.clone()),
        segment_opened_at: SystemTime::UNIX_EPOCH,
        segment_index: 1,
        timing: ChannelTiming {
            finish: ChannelSegmentFinish::Formatter,
            format_wait: Duration::from_secs(3),
            ..ChannelTiming::default()
        },
        segment_source: Some(segment.source.clone()),
        emitter: Some(segment.parts.emitter.clone()),
        delivery_turn: Arc::new(StdMutex::new(None)),
        mode: ChannelOpenMode::Live,
        bus: Some(segment.source.bus.clone()),
        last_text: segment.parts.last_text.clone(),
        refinement_warnings: segment.parts.refinement_warnings.clone(),
        transcript_bus: Some(segment.parts.transcript_bus.clone()),
        capture_ledger: Some(ledger),
        capture_session_id: Some(p.capture_session_id.clone()),
        participation: Some(p),
    }
}

fn raw(segment: &Segment) -> Vec<String> {
    segment
        .parts
        .emitter
        .raw_entries_for_test()
        .into_iter()
        .flat_map(|entry| {
            entry
                .label
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn rows(segment: &Segment) -> Vec<serde_json::Value> {
    crate::durable_bus_oracle::logical_text(&segment.source.bus)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn mutation(
    ledger: &Arc<StdMutex<AcousticLedger>>,
    capture: &str,
    start: u64,
    end: u64,
    generation: u64,
    label: &str,
) -> EngineEvent {
    let occurrence = OccurrenceIdentity::new(capture, 1, start, end);
    let mut ledger = ledger.lock().unwrap();
    if !ledger.is_qualified(&occurrence) {
        let calibration = EnergyCalibration::new("shared-falsifier", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(start),
            vad_close_sample: Some(end),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
    }
    let observation = ObservationIdentity::new(
        ObservationProducer::Whisper,
        start + 1,
        generation,
        occurrence,
    );
    let receipt = ledger.admit(&observation, label);
    assert!(
        receipt.grants_mutation(),
        "fixture must enter the reducer: {receipt:?}"
    );
    EngineEvent::LedgerMutation {
        observation,
        label: label.into(),
        receipt,
    }
}

fn attach(
    recorder: &mut StreamingRecorder,
    demux: &Arc<ChannelDemux>,
    segment: &Segment,
    start: u64,
    owner: Option<ChannelParticipantOwner>,
) -> ChannelParticipation {
    let id = recorder.register_channel_feed().id;
    let seat = demux.attach(id, owner, segment.parts.emitter.clone(), start);
    ChannelParticipation {
        id,
        start_sample: start,
        seat,
        capture_session_id: demux.capture_session_id().into(),
        demux: demux.clone(),
    }
}

#[tokio::test]
async fn f9_agent_channel_overlapping_seats_dedup_observations_but_keep_repeated_words() {
    let root = tempfile::tempdir().unwrap();
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let capture = "agent-channels-f9";
    let a = segment(
        root.path(),
        "agent-channel-2-f9",
        2,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let b = segment(
        root.path(),
        "agent-channel-0-f9",
        0,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let demux = ChannelDemux::new(capture.into());
    let mut recorder = StreamingRecorder::new().unwrap();
    attach(&mut recorder, &demux, &a, 0, None);
    attach(&mut recorder, &demux, &b, 0, None);
    demux.on_capture_opened(capture, 1);
    let first = mutation(&ledger, capture, 0, 100, 1, "echo");
    let second = mutation(&ledger, capture, 200, 300, 1, "echo");
    for event in [&first, &second] {
        demux.on_event(event);
        let before = (
            a.parts.emitter.visible_canvas_snapshot().unwrap().revision,
            b.parts.emitter.visible_canvas_snapshot().unwrap().revision,
        );
        demux.on_event(event);
        assert_eq!(
            (
                a.parts.emitter.visible_canvas_snapshot().unwrap().revision,
                b.parts.emitter.visible_canvas_snapshot().unwrap().revision
            ),
            before,
            "one observation must produce no duplicate Raw revision on either seat"
        );
    }
    assert_eq!(raw(&a), ["echo", "echo"]);
    assert_eq!(raw(&b), ["echo", "echo"]);
    for seat in [&a, &b] {
        let starts: Vec<_> = rows(seat)
            .into_iter()
            .filter(|r| r["reducer_action"] == "apply_ledger_decision")
            .map(|r| {
                (
                    r["sample_start"].as_u64().unwrap(),
                    r["sample_end"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(starts, [(0, 100), (200, 300)]);
    }
    demux.on_event(&mutation(&ledger, capture, 0, 100, 2, "corrected"));
    assert_eq!(raw(&a), ["corrected", "echo"]);
    assert_eq!(raw(&b), ["corrected", "echo"]);
    assert_eq!(ledger.lock().unwrap().qualified_occurrences().count(), 2);
}

#[tokio::test(start_paused = true)]
async fn f10_agent_channel_formatter_fallbacks_and_cross_seat_independence() {
    for reason in [
        "formatter_timeout",
        "formatter_failed",
        "no_formatter_authority",
        "no_terminal_document",
    ] {
        let root = tempfile::tempdir().unwrap();
        let capture = format!("agent-channels-{reason}");
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let a = segment(
            root.path(),
            &format!("agent-channel-2-{reason}"),
            2,
            &capture,
            ledger.clone(),
            DeliverySeat::Primary,
        );
        let b = segment(
            root.path(),
            &format!("agent-channel-0-{reason}"),
            0,
            &capture,
            ledger.clone(),
            DeliverySeat::Primary,
        );
        let demux = ChannelDemux::new(capture.clone());
        let mut recorder = StreamingRecorder::new().unwrap();
        let pa = attach(&mut recorder, &demux, &a, 0, None);
        let pb = attach(&mut recorder, &demux, &b, 0, None);
        demux.on_capture_opened(&capture, 1);
        for (i, text) in ["one", "two", "three"].into_iter().enumerate() {
            demux.on_event(&mutation(
                &ledger,
                &capture,
                i as u64 * 200,
                i as u64 * 200 + 100,
                1,
                text,
            ));
        }
        assert_eq!(raw(&a), ["one", "two", "three"]);
        assert_eq!(raw(&b), ["one", "two", "three"]);
        demux.detach(pa.id, 600);
        demux.on_event(&EngineEvent::DecoderCovered {
            session_id: capture.clone(),
            capture_epoch: 1,
            sample_end: 600,
        });
        assert_eq!(
            demux.settle(pa.id, Duration::ZERO).await,
            DetachSettlement::DecoderPassed
        );
        let mut oa = open(&a, pa, ledger.clone());
        if reason != "no_terminal_document" {
            a.parts.emitter.note_channel_detached();
        }
        if reason == "no_formatter_authority" {
            oa.emitter = None;
        }
        let sender = if matches!(reason, "formatter_timeout" | "formatter_failed") {
            Some(formatter(&a.source.label))
        } else {
            None
        };
        let mut sender = sender;
        if reason == "formatter_failed" {
            sender
                .take()
                .unwrap()
                .send(AiFormatResult {
                    text: "discard".into(),
                    reasoning_text: None,
                    status: AiFormatStatus::Failed,
                })
                .unwrap();
        }
        let closing = RecordingController::finish_channel_message(
            &oa,
            a.parts.transcript_bus.channel_closing_text(),
        );
        tokio::pin!(closing);
        if reason == "formatter_timeout" {
            assert!(matches!(
                std::future::poll_fn(|cx| Poll::Ready(closing.as_mut().poll(cx))).await,
                Poll::Pending
            ));
        }
        // A's formatter may still be pending: B accepts more Raw and seals.
        demux.on_event(&mutation(&ledger, &capture, 700, 800, 1, "four"));
        assert_eq!(raw(&a), ["one", "two", "three"]);
        assert_eq!(raw(&b), ["one", "two", "three", "four"]);
        let mut ob = open(&b, pb, ledger.clone());
        ob.timing.finish = ChannelSegmentFinish::LightPlus;
        let delivered_b = RecordingController::finish_channel_message(
            &ob,
            b.parts.transcript_bus.channel_closing_text(),
        )
        .await
        .unwrap();
        assert_eq!(delivered_b.text, "one. Two. Three. Four.");
        b.parts
            .transcript_bus
            .publish_channel_closed(0, Some(&delivered_b))
            .unwrap();
        assert_eq!(
            rows(&b)
                .iter()
                .filter(|r| r["status"] == "transcript_sealed"
                    && r["schema"] == "codescribe.transcript-evidence.v1")
                .count(),
            1
        );
        if reason == "formatter_timeout" {
            tokio::time::advance(Duration::from_secs(4)).await;
        }
        let delivered_a = closing.await.unwrap();
        assert_eq!(delivered_a.text, "One. Two. Three.");
        assert_eq!(delivered_a.revision, "light_plus");
        assert_eq!(delivered_a.fallback, Some(reason));
        let held = ledger.lock().unwrap();
        assert_eq!(held.qualified_occurrences().count(), 4);
        assert_eq!(
            held.qualified_occurrences()
                .filter(|occurrence| held.is_sealed(occurrence))
                .count(),
            0
        );
        drop(sender);
    }
}

struct SessionDrop(Arc<AtomicU64>);
impl Drop for SessionDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn f13_agent_channel_user_path_2_02_0_off_keeps_one_recognizer_and_exact_raw() {
    // The real RecordingController retains the same recorder throughout.
    let controller = RecordingController::new_without_keychain();
    let mut slot = controller.recorder.lock().await;
    *slot = Some(StreamingRecorder::new().unwrap());
    let recorder = slot.as_mut().unwrap();
    let root = tempfile::tempdir().unwrap();
    let capture = "agent-channels-f13";
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let a = segment(
        root.path(),
        "agent-channel-2-f13",
        2,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let b = segment(
        root.path(),
        "agent-channel-0-f13",
        0,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let starts = Arc::new(AtomicU64::new(0));
    let exits = Arc::new(AtomicU64::new(0));
    let receipts = Arc::new(StdMutex::new(Vec::new()));
    let (ack_tx, mut ack_rx) = mpsc::unbounded_channel();
    let (sc, ec, rc) = (starts.clone(), exits.clone(), receipts.clone());
    let expected_ledger = ledger.clone();
    recorder.set_channel_recognizer_factory(Arc::new(move |mut pcm, sink, mut config| {
        let (sc, ec, rc, ack_tx, expected_ledger) = (
            sc.clone(),
            ec.clone(),
            rc.clone(),
            ack_tx.clone(),
            expected_ledger.clone(),
        );
        Box::pin(async move {
            sc.fetch_add(1, Ordering::SeqCst);
            let _exit = SessionDrop(ec);
            assert!(Arc::ptr_eq(&config.acoustic_ledger, &expected_ledger));
            let mut sample_end = 0;
            while let Some(block) = pcm.recv().await {
                sample_end += block.len() as u64;
                let words: &[(u64, u64, &str)] = match sample_end {
                    4_000 => &[(1_000, 2_000, "before")],
                    7_000 => &[(3_900, 4_200, "join"), (5_000, 6_000, "shared")],
                    9_000 => &[(6_800, 7_100, "tail"), (8_000, 8_500, "after")],
                    10_000 => &[(9_500, 9_900, "last")],
                    _ => panic!("unexpected feed boundary {sample_end}"),
                };
                for &(start, end, label) in words {
                    sink.on_event(&mutation(
                        &config.acoustic_ledger,
                        &config.session_id,
                        start,
                        end,
                        1,
                        label,
                    ));
                }
                // Ordered after all eligible text, as the production session.
                sink.on_event(&EngineEvent::DecoderCovered {
                    session_id: config.session_id.clone(),
                    capture_epoch: 1,
                    sample_end,
                });
                ack_tx.send(sample_end).unwrap();
            }
            let terminal = config.terminal_audio.take().unwrap();
            let receipt = tokio::task::spawn_blocking(move || terminal.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            rc.lock().unwrap().push(receipt);
            config.last_window_closed.take().unwrap().send(()).unwrap();
            drop(config.lifecycle_events.take());
        })
    }));
    let pa = recorder
        .begin_channel_session_for_test(ChannelSessionSpec {
            session_id: capture.into(),
            runtime_settings: a.source.runtime_settings.clone(),
            language: None,
            acoustic_ledger: ledger.clone(),
            participant: ChannelParticipantOpen {
                owner: participant_owner(2, Some("codex"), Some("f13-owner")),
                sink: a.parts.emitter.clone(),
            },
        })
        .unwrap();
    recorder.offer_channel_pcm_for_test(&vec![0.25; 4_000]);
    assert_eq!(ack_rx.recv().await, Some(4_000));
    let pb = recorder
        .join_channel_session(ChannelParticipantOpen {
            owner: None,
            sink: b.parts.emitter.clone(),
        })
        .unwrap();
    assert_eq!(pb.start_sample, 4_000);
    assert_eq!(raw(&a), ["before"]);
    assert_eq!(raw(&b), Vec::<String>::new());
    recorder.offer_channel_pcm_for_test(&vec![0.25; 3_000]);
    assert_eq!(ack_rx.recv().await, Some(7_000));
    assert_eq!(&raw(&a)[1..], raw(&b)); // exact token lists, not substring similarity
    assert_eq!(raw(&b), ["join", "shared"]);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.capture_subscriber_count(), 1);
    let original = recorder.shared_channel_view().unwrap();
    let close_a = recorder.close_channel_session(pa.id).await;
    assert!(!close_a.released_stream());
    assert_eq!(recorder.capture_subscriber_count(), 1);
    assert_eq!(recorder.channel_participant_count(), 1);
    assert!(Arc::ptr_eq(
        &original.acoustic_ledger,
        &recorder.shared_channel_view().unwrap().acoustic_ledger
    ));
    assert!(Arc::ptr_eq(
        &original.demux,
        &recorder.shared_channel_view().unwrap().demux
    ));
    recorder.offer_channel_pcm_for_test(&vec![0.25; 2_000]);
    assert_eq!(ack_rx.recv().await, Some(9_000));
    let mut oa = open(&a, pa.clone(), ledger.clone());
    oa.timing.finish = ChannelSegmentFinish::LightPlus;
    // Real detached message completion, while the companion's stream is live.
    RecordingController::settle_closed_channel(
        2,
        oa,
        ChannelSealReason::Hangup,
        ChannelClose::Session,
        SettleTail {
            shared_bus: root.path().join("shared.jsonl"),
            closed: Some(close_a),
            retained_capture: None,
            order: DeliveryOrder {
                prior: None,
                done: None,
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(recorder.channel_participant_count(), 1);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(exits.load(Ordering::SeqCst), 0);
    assert_eq!(receipts.lock().unwrap().len(), 0);
    assert_eq!(
        rows(&a).iter().filter(|r| r["state"] == "sealed").count(),
        1
    );
    let sealed_a = rows(&a)
        .into_iter()
        .find(|r| r["state"] == "sealed")
        .unwrap();
    assert_eq!(
        sealed_a["participation"],
        serde_json::json!({"capture_session_id":capture,
        "start_sample":0, "end_sample":7_000, "delivery_seat":"primary"})
    );
    assert_eq!(raw(&b), ["join", "shared", "tail", "after"]);
    assert_eq!(
        rows(&a)
            .iter()
            .find(|r| r["status"] == "transcript_sealed")
            .unwrap()["rendered_text"],
        "Before. Join. Shared. Tail."
    );
    recorder.offer_channel_pcm_for_test(&vec![0.25; 1_000]);
    assert_eq!(ack_rx.recv().await, Some(10_000));
    assert_eq!(raw(&b), ["join", "shared", "tail", "after", "last"]);
    let closed = recorder.close_channel_session(pb.id).await;
    assert!(closed.released_stream());
    let (released, wav) = closed.finish().await.unwrap();
    assert!(released);
    let wav = wav.unwrap();
    assert_eq!(hound_duration(&wav), 10_000);
    assert_eq!(recorder.capture_subscriber_count(), 0);
    assert_eq!(recorder.channel_participant_count(), 0);
    assert!(recorder.shared_channel_view().is_none());
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    let archives = receipts.lock().unwrap();
    assert_eq!(archives.len(), 1);
    assert_eq!(ledger.lock().unwrap().qualified_occurrences().count(), 6);
    let intervals = pa.demux.intervals();
    assert_eq!(
        intervals
            .iter()
            .map(|i| (i.start_sample, i.end_sample))
            .collect::<Vec<_>>(),
        [(0, Some(7_000)), (4_000, Some(10_000))]
    );
    std::fs::remove_file(wav).unwrap();
}

fn hound_duration(path: &Path) -> u64 {
    // Read the native WAV header through the existing audio loader; no device.
    let (pcm, _) = codescribe_core::audio::load_audio_file(path).unwrap();
    pcm.len() as u64
}

#[tokio::test]
async fn f3_agent_channel_sealed_rows_keep_bus_capture_seat_and_refusal_identity() {
    let root = tempfile::tempdir().unwrap();
    let capture = "agent-channels-f3";
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let a = segment(
        root.path(),
        "agent-channel-2-f3",
        2,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let b = segment(
        root.path(),
        "agent-channel-0-f3",
        0,
        capture,
        ledger.clone(),
        DeliverySeat::Projection,
    );
    let demux = ChannelDemux::new(capture.into());
    let mut recorder = StreamingRecorder::new().unwrap();
    let pa = attach(
        &mut recorder,
        &demux,
        &a,
        0,
        participant_owner(2, Some("codex"), Some("same-session")),
    );
    let pb = attach(
        &mut recorder,
        &demux,
        &b,
        0,
        participant_owner(0, Some("codex"), Some("same-session")),
    );
    assert_eq!(
        (pa.seat, pb.seat),
        (DeliverySeat::Primary, DeliverySeat::Projection)
    );
    a.parts.transcript_bus.publish_started();
    b.parts.transcript_bus.publish_started();
    demux.on_capture_opened(capture, 1);
    demux.on_event(&mutation(&ledger, capture, 0, 100, 1, "shared"));
    let capture_refusal = ledger
        .lock()
        .unwrap()
        .terminal_finality(capture, 1)
        .into_refusal()
        .unwrap();
    let bus_refusal = AcousticLedger::new()
        .terminal_finality(&a.source.label, 1)
        .into_refusal()
        .unwrap();
    let closing_text = a.parts.transcript_bus.channel_closing_text().unwrap();
    assert!(
        a.parts
            .transcript_bus
            .matches_refused_document(&bus_refusal, &closing_text)
    );
    assert!(
        !a.parts
            .transcript_bus
            .matches_refused_document(&capture_refusal, &closing_text)
    );
    assert!(
        !b.parts
            .transcript_bus
            .matches_refused_document(&bus_refusal, &closing_text)
    );
    demux.detach(pa.id, 200);
    demux.detach(pb.id, 200);
    for (digit, s, p) in [(2, &a, pa), (0, &b, pb)] {
        let mut o = open(s, p, ledger.clone());
        o.timing.finish = ChannelSegmentFinish::LightPlus;
        RecordingController::settle_closed_channel(
            digit,
            o,
            ChannelSealReason::Hangup,
            ChannelClose::Session,
            SettleTail {
                shared_bus: root.path().join("shared.jsonl"),
                closed: None,
                retained_capture: None,
                order: DeliveryOrder {
                    prior: None,
                    done: None,
                },
            },
        )
        .await
        .unwrap();
        let all = rows(s);
        let sealed = all.iter().find(|r| r["state"] == "sealed").unwrap();
        assert_eq!(sealed["session_id"], s.source.label);
        assert_eq!(sealed["participation"]["capture_session_id"], capture);
        assert_eq!(
            sealed["participation"]["delivery_seat"],
            if digit == 2 { "primary" } else { "projection" }
        );
        assert_eq!(sealed["participation"]["start_sample"], 0);
        assert_eq!(sealed["participation"]["end_sample"], 200);
    }
    // A capture refusal never impersonates either distinct bus document.
    let refusal = ledger
        .lock()
        .unwrap()
        .terminal_finality(capture, 1)
        .into_refusal()
        .unwrap();
    assert!(
        !a.parts
            .transcript_bus
            .matches_refused_document(&refusal, "shared")
    );
    assert!(
        !b.parts
            .transcript_bus
            .matches_refused_document(&refusal, "shared")
    );
    assert_ne!(a.source.label, b.source.label);
}

#[tokio::test]
async fn f10_agent_channel_foreign_terminal_seal_is_refused_without_rewriting_words() {
    let root = tempfile::tempdir().unwrap();
    let capture = "agent-channels-foreign-seal";
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let a = segment(
        root.path(),
        "agent-channel-2-foreign",
        2,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let b = segment(
        root.path(),
        "agent-channel-0-foreign",
        0,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let demux = ChannelDemux::new(capture.into());
    let mut recorder = StreamingRecorder::new().unwrap();
    let pa = attach(&mut recorder, &demux, &a, 0, None);
    demux.on_capture_opened(capture, 1);
    demux.on_event(&mutation(&ledger, capture, 0, 100, 1, "first"));
    demux.detach(pa.id, 200);
    attach(&mut recorder, &demux, &b, 200, None);
    demux.on_event(&mutation(&ledger, capture, 200, 300, 1, "second"));
    let before_a = a.parts.emitter.visible_canvas_snapshot().unwrap().revision;
    let before_b = b.parts.emitter.visible_canvas_snapshot().unwrap().revision;
    let seal = {
        let mut held = ledger.lock().unwrap();
        let owners = held.qualified_occurrences().cloned().collect::<Vec<_>>();
        for owner in owners {
            held.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
            assert!(held.note_frontier_return(&owner, ObservationProducer::Whisper));
        }
        held.seal_terminal(capture, 1).unwrap()
    };
    assert_eq!(seal.sealed_occurrences.len(), 2);
    demux.on_event(&EngineEvent::LedgerSeal { receipt: seal });
    assert_eq!(
        (
            a.parts.emitter.visible_canvas_snapshot().unwrap().revision,
            b.parts.emitter.visible_canvas_snapshot().unwrap().revision,
            b.parts.emitter.terminal_formatter_request().is_some()
        ),
        (before_a, before_b, false),
        "a foreign terminal seal must neither revise nor end either participant",
    );
    assert_eq!(raw(&a), ["first"]);
    assert_eq!(raw(&b), ["second"]);
    assert!(b.parts.emitter.terminal_formatter_request().is_none());
    let owner = OccurrenceIdentity::new(capture, 1, 0, 100);
    let mut held = ledger.lock().unwrap();
    let refused = held.admit(
        &ObservationIdentity::new(ObservationProducer::Whisper, 99, 99, owner.clone()),
        "rewrite",
    );
    assert!(!refused.grants_mutation());
    assert_eq!(held.text_of(&owner), Some("first"));
}

#[tokio::test]
async fn f12_agent_channel_transcript_bus_rows_stay_constant_for_one_and_two_seats() {
    use codescribe_core::audio::capture_receipt::{
        AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
    };
    use codescribe_core::stt::tail_provider::TailSampleRange;
    for seats in [1, 2] {
        for decisions in [1, 17, 211] {
            let root = tempfile::tempdir().unwrap();
            let capture = format!("agent-channels-quiet-{seats}-{decisions}");
            let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
            let demux = ChannelDemux::new(capture.clone());
            let mut recorder = StreamingRecorder::new().unwrap();
            let documents: Vec<_> = (0..seats)
                .map(|digit| {
                    segment(
                        root.path(),
                        &format!("agent-channel-{digit}-quiet-{decisions}"),
                        digit,
                        &capture,
                        ledger.clone(),
                        DeliverySeat::Primary,
                    )
                })
                .collect();
            for document in &documents {
                attach(&mut recorder, &demux, document, 0, None);
            }
            demux.on_capture_opened(&capture, 1);
            for generation in 1..=decisions {
                demux.on_event(&mutation(
                    &ledger,
                    &capture,
                    0,
                    1_000,
                    generation,
                    &format!("Token-{generation}."),
                ));
                for document in &documents {
                    assert_eq!(raw(document), [format!("Token-{generation}.")]);
                    assert!(document.parts.emitter.wait_paint_published().await);
                    assert_eq!(
                        document.paints.lock().unwrap().last().unwrap(),
                        &format!("Token-{generation}.")
                    );
                }
            }
            let speech = AcousticSpeechEvidence::measured(
                CaptureEvidenceIdentity::new(&capture, 1),
                codescribe_core::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER,
                AcousticAvailability::Observed {
                    observed_samples: 1_000,
                },
                vec![TailSampleRange {
                    session: capture.clone(),
                    capture_epoch: 1,
                    sample_start: 0,
                    sample_end: 1_000,
                }],
            );
            let (coverage, seal) = {
                let mut held = ledger.lock().unwrap();
                let coverage = held.assess_seal_coverage(&capture, 1, &speech, 1);
                assert!(coverage.status.is_complete(), "{coverage:?}");
                assert_eq!(
                    (coverage.speech_samples, coverage.covered_samples),
                    (1_000, 1_000)
                );
                assert!(held.record_seal_coverage(coverage.clone()));
                let owner = OccurrenceIdentity::new(&capture, 1, 0, 1_000);
                held.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
                assert!(held.note_frontier_return(&owner, ObservationProducer::Whisper));
                (coverage, held.seal_terminal(&capture, 1).unwrap())
            };
            demux.on_event(&EngineEvent::SealCoverage {
                receipt: coverage,
                comparison: None,
            });
            demux.on_event(&EngineEvent::LedgerSeal { receipt: seal });
            for document in &documents {
                let revision = document
                    .parts
                    .emitter
                    .visible_canvas_snapshot()
                    .unwrap()
                    .revision;
                document
                    .parts
                    .emitter
                    .apply_user_revision(UserRevisionIntent {
                        session_id: capture.clone(),
                        source_revision: revision,
                        rendered_text: "Edited.".into(),
                        provenance: DocumentRevisionProvenance::UserEdit,
                    })
                    .unwrap();
                let all = rows(document);
                let mut counts = std::collections::BTreeMap::new();
                for row in &all {
                    if let Some(action) = row["reducer_action"].as_str() {
                        *counts.entry(action).or_insert(0) += 1;
                    }
                }
                assert_eq!(
                    counts,
                    std::collections::BTreeMap::from([
                        ("apply_ledger_decision", 1),
                        ("record_ledger_terminal_seal", 1),
                        ("seal_coverage", 1),
                        ("apply_manual_edit", 1),
                    ]),
                    "{seats} seats, {decisions} revisions: {all:?}"
                );
                assert_eq!(all.len(), 5, "recipients plus four evidence rows");
                assert_eq!(
                    all.iter()
                        .filter(|r| r["schema"] == "codescribe.transcript-evidence.v1")
                        .count(),
                    4
                );
                assert_eq!(
                    document
                        .parts
                        .transcript_bus
                        .channel_closing_text()
                        .as_deref(),
                    Some("Edited.")
                );
            }
        }
    }
}

#[tokio::test]
async fn f4_agent_channel_single_seat_keeps_exact_quiet_terminal_text_and_capabilities() {
    let root = tempfile::tempdir().unwrap();
    let capture = "agent-channels-single";
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let document = segment(
        root.path(),
        "agent-channel-2-single",
        2,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    document.parts.emitter.on_capture_opened(capture, 1);
    document
        .parts
        .emitter
        .on_event(&mutation(&ledger, capture, 0, 1_000, 1, "Single channel."));
    let seal = {
        let mut held = ledger.lock().unwrap();
        let owner = OccurrenceIdentity::new(capture, 1, 0, 1_000);
        held.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        assert!(held.note_frontier_return(&owner, ObservationProducer::Whisper));
        held.seal_terminal(capture, 1).unwrap()
    };
    document
        .parts
        .emitter
        .on_event(&EngineEvent::LedgerSeal { receipt: seal });
    assert!(document.parts.emitter.wait_paint_published().await);
    assert_eq!(raw(&document), ["Single", "channel."]);
    let all = rows(&document);
    let evidence: Vec<_> = all
        .iter()
        .filter(|r| r["schema"] == "codescribe.transcript-evidence.v1")
        .collect();
    assert_eq!(evidence.len(), 2);
    assert_eq!(
        evidence
            .iter()
            .map(|r| r["reducer_action"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["apply_ledger_decision", "record_ledger_terminal_seal"]
    );
    for row in evidence {
        assert_eq!(row["session_id"], "agent-channel-2-single");
        assert_eq!(row["occurrence_session_id"], capture);
        assert_eq!(row["rendered_text"], "Single channel.");
        assert_eq!(row["capture_epoch"], 1);
        assert_eq!(row["sample_start"], 0);
        assert_eq!(row["sample_end"], 1_000);
        assert_eq!(row["can_paste"], false);
        assert_eq!(row["can_insert"], false);
        assert_eq!(row["can_send_to_agent"], false);
    }
    assert_eq!(
        document.paints.lock().unwrap().last().unwrap(),
        "Single channel."
    );
    // Control corridor: the original single-channel authority layout has
    // bus session == capture session and no shared-capture override.
    let control_path = root.path().join("unshared-control.jsonl");
    let control_bus = Arc::new(
        TranscriptBus::open_at(
            TranscriptSession {
                session_id: capture.into(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
                audience: Some("seat-2".into()),
                badge_only: true,
            },
            control_path.clone(),
            None,
        )
        .unwrap(),
    );
    control_bus.record_channel_receipt(&serde_json::json!({
        "schema":"codescribe.channel-recipients.v1", "session_id":capture,
        "channel":"2", "recipients":[],
    }));
    let control_ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let control =
        channel_presentation_emitter(control_bus, control_ledger.clone(), &settings(root.path()))
            .with_refinement_warnings(Arc::new(StdMutex::new(Vec::new())));
    control.on_capture_opened(capture, 1);
    control.on_event(&mutation(
        &control_ledger,
        capture,
        0,
        1_000,
        1,
        "Single channel.",
    ));
    let control_seal = {
        let mut held = control_ledger.lock().unwrap();
        let owner = OccurrenceIdentity::new(capture, 1, 0, 1_000);
        held.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        assert!(held.note_frontier_return(&owner, ObservationProducer::Whisper));
        held.seal_terminal(capture, 1).unwrap()
    };
    control.on_event(&EngineEvent::LedgerSeal {
        receipt: control_seal,
    });
    let control_rows = crate::durable_bus_oracle::logical_text(&control_path)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|row| row["schema"] == "codescribe.transcript-evidence.v1")
        .collect::<Vec<_>>();
    let normalize = |mut row: serde_json::Value| {
        row["session_id"] = serde_json::json!("bus-document");
        row.as_object_mut().unwrap().remove("emitted_at");
        row
    };
    let shared_rows = rows(&document)
        .into_iter()
        .filter(|row| row["schema"] == "codescribe.transcript-evidence.v1")
        .map(&normalize)
        .collect::<Vec<_>>();
    assert_eq!(
        shared_rows,
        control_rows.into_iter().map(normalize).collect::<Vec<_>>(),
        "single-seat evidence fields equal the unshared authority corridor, apart from bus ID and emission time"
    );
}

#[tokio::test]
async fn f8_agent_channel_reused_digit_preserves_old_bus_and_never_backfills_new_document() {
    let root = tempfile::tempdir().unwrap();
    let capture = "agent-channels-reused-digit";
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let old = segment(
        root.path(),
        "agent-channel-2-old-lease",
        2,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let new = segment(
        root.path(),
        "agent-channel-2-new-lease",
        2,
        capture,
        ledger.clone(),
        DeliverySeat::Primary,
    );
    let demux = ChannelDemux::new(capture.into());
    let mut recorder = StreamingRecorder::new().unwrap();
    let pa = attach(
        &mut recorder,
        &demux,
        &old,
        0,
        participant_owner(2, Some("codex"), Some("old")),
    );
    demux.on_capture_opened(capture, 1);
    demux.on_event(&mutation(&ledger, capture, 0, 100, 1, "old"));
    demux.detach(pa.id, 200);
    let pb = attach(
        &mut recorder,
        &demux,
        &new,
        200,
        participant_owner(2, Some("codex"), Some("new")),
    );
    // Old decoder work returns while the successor already occupies digit 2.
    demux.on_event(&mutation(&ledger, capture, 100, 180, 1, "tail"));
    demux.on_event(&mutation(&ledger, capture, 220, 300, 1, "new"));
    demux.on_event(&EngineEvent::DecoderCovered {
        session_id: capture.into(),
        capture_epoch: 1,
        sample_end: 300,
    });
    assert_eq!(
        demux.settle(pa.id, Duration::ZERO).await,
        DetachSettlement::DecoderPassed
    );
    demux.on_event(&mutation(&ledger, capture, 0, 100, 2, "late"));
    assert_eq!(raw(&old), ["old", "tail"]);
    assert_eq!(raw(&new), ["new"]);
    assert_ne!(pa.id, pb.id);
    for document in [&old, &new] {
        let all = rows(document);
        assert_eq!(
            all.iter()
                .filter(|r| r["schema"] == "codescribe.channel-recipients.v1")
                .count(),
            1
        );
        assert!(all.iter().all(|r| r["session_id"] == document.source.label));
    }
    assert_eq!(
        rows(&old)
            .iter()
            .filter(|r| r["reducer_action"] == "apply_ledger_decision")
            .count(),
        2
    );
    assert_eq!(
        rows(&new)
            .iter()
            .filter(|r| r["reducer_action"] == "apply_ledger_decision")
            .count(),
        1
    );
}
