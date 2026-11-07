//! Resource and lifetime falsifiers over the real registry and TakeArchive.
use super::*;
use crate::config::{CapturedRuntimeInputs, Config};

struct NullSink;
impl EventSink for NullSink {
    fn on_event(&self, _: &EngineEvent) {}
}

struct DropCount(Arc<AtomicU64>);
impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fake {
    starts: Arc<AtomicU64>,
    drops: Arc<AtomicU64>,
    receipts: Arc<AtomicU64>,
    started: mpsc::UnboundedReceiver<Arc<StdMutex<AcousticLedger>>>,
    fed: mpsc::UnboundedReceiver<u64>,
    eof: mpsc::UnboundedReceiver<()>,
}

fn fake(recorder: &mut StreamingRecorder, panic_after_receipt: bool) -> Fake {
    let starts = Arc::new(AtomicU64::new(0));
    let drops = Arc::new(AtomicU64::new(0));
    let receipts = Arc::new(AtomicU64::new(0));
    let (start_tx, started) = mpsc::unbounded_channel();
    let (feed_tx, fed) = mpsc::unbounded_channel();
    let (eof_tx, eof) = mpsc::unbounded_channel();
    let (sc, dc, rc) = (starts.clone(), drops.clone(), receipts.clone());
    recorder.set_channel_recognizer_factory(Arc::new(move |mut feed, _sink, mut config| {
        let (sc, dc, rc) = (sc.clone(), dc.clone(), rc.clone());
        let (start_tx, feed_tx, eof_tx) = (start_tx.clone(), feed_tx.clone(), eof_tx.clone());
        sc.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let _lifetime = DropCount(dc);
            start_tx.send(config.acoustic_ledger.clone()).unwrap();
            let mut samples = 0;
            while let Some(block) = feed.recv().await {
                samples += block.len() as u64;
                feed_tx.send(samples).unwrap();
            }
            eof_tx.send(()).unwrap();
            let terminal = config
                .terminal_audio
                .take()
                .expect("production archive receiver");
            let receipt = tokio::task::spawn_blocking(move || terminal.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(receipt.sample_count, samples);
            assert_eq!(
                hound::WavReader::open(&receipt.path).unwrap().duration() as u64,
                samples
            );
            rc.fetch_add(1, Ordering::SeqCst);
            config.last_window_closed.take().unwrap().send(()).unwrap();
            drop(config.lifecycle_events.take());
            assert!(
                !panic_after_receipt,
                "injected recognizer failure after complete drain"
            );
        })
    }));
    Fake {
        starts,
        drops,
        receipts,
        started,
        fed,
        eof,
    }
}

fn spec(
    root: &std::path::Path,
    capture: &str,
    ledger: Arc<StdMutex<AcousticLedger>>,
) -> ChannelSessionSpec {
    ChannelSessionSpec {
        session_id: capture.into(),
        runtime_settings: Arc::new(Config::runtime_snapshot_from_captured(
            CapturedRuntimeInputs::defaults_at(root.to_owned(), 7),
        )),
        language: None,
        acoustic_ledger: ledger,
        participant: ChannelParticipantOpen {
            owner: None,
            sink: Arc::new(NullSink),
        },
    }
}

async fn feed(recorder: &StreamingRecorder, fake: &mut Fake, count: usize, expected: u64) {
    recorder.offer_channel_pcm_for_test(&vec![0.25; count]);
    assert_eq!(fake.fed.recv().await, Some(expected));
}

#[tokio::test]
#[serial_test::serial]
async fn f5_streaming_recorder_n_joins_create_one_session_ledger_archive_and_wav() {
    let root = tempfile::tempdir().unwrap();
    let mut recorder = StreamingRecorder::new().unwrap();
    let mut fake = fake(&mut recorder, false);
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let first = recorder
        .begin_channel_session_for_test(spec(root.path(), "agent-channels-f5", ledger.clone()))
        .unwrap();
    assert!(Arc::ptr_eq(&fake.started.recv().await.unwrap(), &ledger));
    let mut seats = vec![first.id];
    for _ in 0..8 {
        seats.push(
            recorder
                .join_channel_session(ChannelParticipantOpen {
                    owner: None,
                    sink: Arc::new(NullSink),
                })
                .unwrap()
                .id,
        );
    }
    feed(&recorder, &mut fake, 160, 160).await;
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.capture_subscriber_count(), 1);
    assert_eq!(recorder.channel_participant_count(), 9);
    assert_eq!(
        recorder
            .pcm_feeds
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f.archive.is_some())
            .count(),
        1
    );
    assert!(
        recorder
            .shared_channel
            .as_ref()
            .unwrap()
            .task
            .archive
            .is_some()
    );
    assert!(Arc::ptr_eq(
        &recorder.shared_channel_view().unwrap().acoustic_ledger,
        &ledger
    ));
    let mut closing = Vec::new();
    for seat in seats.iter().take(8) {
        let closed = recorder.close_channel_session(*seat).await;
        assert!(!closed.released_stream());
        closing.push(closed);
        assert_eq!(recorder.capture_subscriber_count(), 1);
    }
    let last = recorder.close_channel_session(seats[8]).await;
    assert!(last.released_stream());
    let (released, path) = last.finish().await.unwrap();
    assert!(released);
    let path = path.unwrap();
    assert_eq!(hound::WavReader::open(&path).unwrap().duration(), 160);
    for closed in closing {
        assert_eq!(closed.finish().await.unwrap(), (false, None));
    }
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
    assert_eq!(fake.receipts.load(Ordering::SeqCst), 1);
    assert_eq!(fake.drops.load(Ordering::SeqCst), 1);
    assert_eq!(recorder.capture_subscriber_count(), 0);
    assert_eq!(recorder.channel_participant_count(), 0);
    assert!(recorder.shared_channel_view().is_none());
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn f4_streaming_recorder_lone_turnover_ends_old_capture_and_starts_one_fresh_stream() {
    let root = tempfile::tempdir().unwrap();
    let mut recorder = StreamingRecorder::new().unwrap();
    let mut fake = fake(&mut recorder, false);
    let old_ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let first = recorder
        .begin_channel_session_for_test(spec(root.path(), "agent-channels-old", old_ledger.clone()))
        .unwrap();
    assert!(Arc::ptr_eq(
        &fake.started.recv().await.unwrap(),
        &old_ledger
    ));
    feed(&recorder, &mut fake, 100, 100).await;
    assert!(recorder.channel_rotation_turns_over(first.id));
    let next_ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let (next, old) = recorder
        .rotate_channel_segment(
            first.id,
            spec(root.path(), "agent-channels-new", next_ledger.clone()),
            1.0,
        )
        .await
        .unwrap();
    assert!(old.released_stream());
    assert!(Arc::ptr_eq(
        &fake.started.recv().await.unwrap(),
        &next_ledger
    ));
    assert_eq!(next.start_sample, 0);
    assert_eq!(next.capture_session_id, "agent-channels-new");
    assert_eq!(recorder.capture_subscriber_count(), 1);
    let (physical, old_path) = old.finish().await.unwrap();
    assert!(!physical);
    let old_path = old_path.unwrap();
    assert_eq!(hound::WavReader::open(&old_path).unwrap().duration(), 100);
    feed(&recorder, &mut fake, 60, 60).await;
    let (_, new_path) = recorder
        .close_channel_session(next.id)
        .await
        .finish()
        .await
        .unwrap();
    let new_path = new_path.unwrap();
    assert_ne!(old_path, new_path);
    assert_eq!(hound::WavReader::open(&new_path).unwrap().duration(), 60);
    assert_eq!(fake.starts.load(Ordering::SeqCst), 2);
    assert_eq!(fake.receipts.load(Ordering::SeqCst), 2);
    assert_eq!(fake.drops.load(Ordering::SeqCst), 2);
    std::fs::remove_file(old_path).unwrap();
    std::fs::remove_file(new_path).unwrap();
}

async fn last_failure(cancel: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut recorder = StreamingRecorder::new().unwrap();
    let mut fake = fake(&mut recorder, !cancel);
    use crate::pipeline::acoustic_ledger::{
        AcousticEvidence, EnergyCalibration, ObservationIdentity, ObservationProducer,
        OccurrenceIdentity,
    };
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let occurrence = OccurrenceIdentity::new("agent-channels-f7", 1, 0, 80);
    let seal_id = {
        let mut held = ledger.lock().unwrap();
        let calibration = EnergyCalibration::new("f7", 1.0, 1);
        assert!(
            held.qualify(
                &AcousticEvidence {
                    occurrence: occurrence.clone(),
                    duration_ms: 80.0,
                    energy_integral: 10.0,
                    mean_rms_dbfs: -12.0,
                    peak_dbfs: -3.0,
                    vad_open_sample: Some(0),
                    vad_close_sample: Some(80),
                    evidence_calibration_version: calibration.version.clone(),
                },
                &calibration
            )
            .is_qualified()
        );
        assert!(
            held.admit(
                &ObservationIdentity::new(ObservationProducer::Whisper, 1, 1, occurrence.clone()),
                "immutable"
            )
            .grants_mutation()
        );
        held.schedule_frontier(occurrence.clone(), [ObservationProducer::Whisper]);
        assert!(held.note_frontier_return(&occurrence, ObservationProducer::Whisper));
        held.seal_terminal("agent-channels-f7", 1)
            .unwrap()
            .receipt_id
    };
    let participation = recorder
        .begin_channel_session_for_test(spec(root.path(), "agent-channels-f7", ledger.clone()))
        .unwrap();
    fake.started.recv().await.unwrap();
    feed(&recorder, &mut fake, 80, 80).await;
    let mut closed = recorder.close_channel_session(participation.id).await;
    assert!(closed.released_stream());
    if cancel {
        fake.eof.recv().await.unwrap();
        closed.task.as_mut().unwrap().task.abort();
    }
    let error = closed.finish().await.unwrap_err();
    let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
    assert!(
        failure
            .cause
            .to_string()
            .contains("channel transcription task failed")
    );
    let wav = failure.audio_path.as_ref().unwrap();
    assert_eq!(hound::WavReader::open(wav).unwrap().duration(), 80);
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
    assert_eq!(fake.drops.load(Ordering::SeqCst), 1);
    assert_eq!(fake.receipts.load(Ordering::SeqCst), u64::from(!cancel));
    assert_eq!(recorder.capture_subscriber_count(), 0);
    assert_eq!(recorder.channel_participant_count(), 0);
    assert_eq!(participation.demux.attached_count(), 0);
    {
        let mut held = ledger.lock().unwrap();
        assert_eq!(held.text_of(&occurrence), Some("immutable"));
        assert!(held.is_sealed(&occurrence));
        assert_eq!(
            held.seal_terminal("agent-channels-f7", 1)
                .unwrap()
                .receipt_id,
            seal_id
        );
        assert!(
            !held
                .admit(
                    &ObservationIdentity::new(
                        ObservationProducer::Whisper,
                        2,
                        2,
                        occurrence.clone()
                    ),
                    "rewrite"
                )
                .grants_mutation()
        );
        assert_eq!(held.text_of(&occurrence), Some("immutable"));
    }
    std::fs::remove_file(wav).unwrap();
    assert_eq!(
        recorder
            .close_channel_session(participation.id)
            .await
            .finish()
            .await
            .unwrap(),
        (false, None)
    );
}

#[tokio::test]
#[serial_test::serial]
async fn f7_streaming_recorder_error_releases_last_seat_exactly_once() {
    last_failure(false).await;
}

#[tokio::test]
#[serial_test::serial]
async fn f7_streaming_recorder_cancellation_releases_last_seat_exactly_once() {
    last_failure(true).await;
}

// ---------------------------------------------------------------------------
// Ordinary dictation as a text participant of the shared stream (Maciej,
// 2026-11-07, relayed by Lena's delivery 806be508: one text demux, no second
// PCM feed or recognizer; a document joins from the current sample).
// ---------------------------------------------------------------------------

/// Stands in for the production emitter: it admits a document's participation
/// close (the contract default refuses) and records the formatter flag it
/// inherits at attach.
struct DocumentSink {
    formatter_pending: Arc<AtomicBool>,
}

impl EventSink for DocumentSink {
    fn on_event(&self, _: &EngineEvent) {}

    fn on_formatter_pending(&self, pending: bool) {
        self.formatter_pending.store(pending, Ordering::SeqCst);
    }

    fn close_participation(&self, _: &str, _: u64) -> anyhow::Result<()> {
        Ok(())
    }

    fn wait_presentation_published<'a>(
        &'a self,
        _: &'a str,
        _: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

fn settings(root: &std::path::Path) -> Arc<RuntimeSettingsSnapshot> {
    Arc::new(Config::runtime_snapshot_from_captured(
        CapturedRuntimeInputs::defaults_at(root.to_owned(), 7),
    ))
}

/// Bind and start a normal document the way a Raw hold or toggle does:
/// `bind_dictation_authority` then the same `start_event_session`. The
/// physical recorder is flagged active so no microphone opens.
async fn start_document(
    recorder: &mut StreamingRecorder,
    root: &std::path::Path,
    document: &str,
) -> Arc<AtomicBool> {
    let formatter_pending = Arc::new(AtomicBool::new(false));
    recorder.set_event_sink(Some(Arc::new(DocumentSink {
        formatter_pending: Arc::clone(&formatter_pending),
    })));
    recorder.bind_dictation_authority(document.to_string(), settings(root));
    recorder.recorder.set_active_for_test(true);
    recorder
        .start_event_session(None)
        .await
        .expect("the document joins the shared text stream");
    formatter_pending
}

/// The controller's Stop in two steps: close the participation, detach the take.
async fn stop_document(recorder: &mut StreamingRecorder) -> (bool, ClosedTake) {
    let last = recorder.close_capture().await;
    let take = recorder.detach_closed_take(last);
    assert!(take.is_shared_document());
    (last, take)
}

fn agent_seat() -> ChannelParticipantOpen {
    ChannelParticipantOpen {
        owner: None,
        sink: Arc::new(NullSink),
    }
}

/// Dictation first: the capture is `dictation-capture-<document>`, never the
/// document's own id. The document stops while the agent continues: it gets
/// text, no WAV, and does not release the lease; the agent's last stop owns
/// the whole capture's WAV. (HANDOFF "mandatory collision case".)
#[tokio::test]
#[serial_test::serial]
async fn dictation_first_document_stops_without_the_wav_and_the_agents_last_stop_owns_it() {
    let root = tempfile::tempdir().unwrap();
    let mut recorder = StreamingRecorder::new().unwrap();
    let mut fake = fake(&mut recorder, false);
    start_document(&mut recorder, root.path(), "doc-d").await;
    fake.started.recv().await.unwrap();
    let shared = recorder.shared_channel_view().expect("one shared source");
    assert_eq!(shared.capture_session_id, "dictation-capture-doc-d");
    assert_eq!(
        recorder.bound_capture_session_id(),
        Some("dictation-capture-doc-d")
    );
    assert_eq!(shared.capture_turn, CaptureTurnIntent::HandsFree);
    assert!(recorder.has_take_subscriber());
    feed(&recorder, &mut fake, 100, 100).await;

    // The agent joins the running recognizer at the current sample.
    let agent = recorder.join_channel_session(agent_seat()).unwrap();
    assert_eq!(agent.start_sample, 100);
    assert_eq!(agent.capture_session_id, "dictation-capture-doc-d");
    feed(&recorder, &mut fake, 60, 160).await;
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1, "one recognizer");
    assert_eq!(recorder.channel_participant_count(), 2);

    // DocD stops first.
    let (last, mut take) = stop_document(&mut recorder).await;
    assert!(!last, "the agent still records");
    assert!(!recorder.has_take_subscriber());
    assert_eq!(recorder.channel_participant_count(), 1);
    assert!(
        recorder.shared_channel_view().is_some(),
        "the physical lease survives the document's stop"
    );
    feed(&recorder, &mut fake, 40, 200).await;

    // The agent's stop is the last one: it releases the stream and its WAV.
    let closed = recorder.close_channel_session(agent.id).await;
    assert!(closed.released_stream());
    let (released, path) = closed.finish().await.unwrap();
    assert!(released);
    let path = path.expect("the capture's WAV");
    assert_eq!(hound::WavReader::open(&path).unwrap().duration(), 200);
    assert_eq!(fake.receipts.load(Ordering::SeqCst), 1);
    assert!(recorder.shared_channel_view().is_none());

    // DocD's settlement after the producer ended: text only, never root.wav.
    let (_, own_wav) = take.finish().await.expect("participation settles");
    assert_eq!(own_wav, None, "a shared interval has no exact WAV alias");
    assert!(take.released_shared_capture().is_none());
    assert!(take.take_shared_capture_archive().is_none());
    assert!(take.is_settled());
    std::fs::remove_file(path).unwrap();
}

/// Agent first: the document joins the agent's recognizer (no second start),
/// inherits the live formatter flag at attach, and runs as `SharedDictation`.
/// The agent leaves first; the document's last stop hands the WAV to the
/// capture namespace, never to the document.
#[tokio::test]
#[serial_test::serial]
async fn agent_first_document_joins_once_and_its_last_stop_hands_the_wav_to_the_capture() {
    let root = tempfile::tempdir().unwrap();
    let mut recorder = StreamingRecorder::new().unwrap();
    let mut fake = fake(&mut recorder, false);
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let agent = recorder
        .begin_channel_session_for_test(spec(root.path(), "agent-channels-a1", ledger))
        .unwrap();
    fake.started.recv().await.unwrap();
    feed(&recorder, &mut fake, 80, 80).await;
    // A formatter job is in flight when the document joins.
    recorder
        .shared_channel_view()
        .unwrap()
        .demux
        .on_formatter_pending(true);

    let formatter_pending = start_document(&mut recorder, root.path(), "doc-e").await;
    assert!(
        formatter_pending.load(Ordering::SeqCst),
        "a joining document inherits the current formatter flag"
    );
    assert_eq!(
        fake.starts.load(Ordering::SeqCst),
        1,
        "no second recognizer"
    );
    assert_eq!(
        recorder.bound_capture_session_id(),
        Some("agent-channels-a1")
    );
    assert_eq!(
        recorder.capture_turn_intent(),
        CaptureTurnIntent::SharedDictation
    );
    assert_eq!(recorder.channel_participant_count(), 2);
    assert!(recorder.has_take_subscriber());
    feed(&recorder, &mut fake, 70, 150).await;

    // The agent leaves first; the document keeps recording.
    let closed = recorder.close_channel_session(agent.id).await;
    assert!(!closed.released_stream());
    assert_eq!(recorder.channel_participant_count(), 1);
    feed(&recorder, &mut fake, 50, 200).await;

    // The document's stop is the last: it releases the stream.
    let (last, mut take) = stop_document(&mut recorder).await;
    assert!(last);
    let (_, own_wav) = take.finish().await.unwrap();
    assert_eq!(
        own_wav, None,
        "two intervals: the document owns no exact WAV"
    );
    assert_eq!(take.released_shared_capture(), Some("agent-channels-a1"));
    let (capture, path) = take
        .take_shared_capture_archive()
        .expect("the capture's archive moves under its own id");
    assert_eq!(capture, "agent-channels-a1");
    assert_eq!(hound::WavReader::open(&path).unwrap().duration(), 200);
    assert!(take.is_settled());
    assert_eq!(closed.finish().await.unwrap(), (false, None));
    assert_eq!(fake.receipts.load(Ordering::SeqCst), 1);
    assert!(recorder.shared_channel_view().is_none());
    std::fs::remove_file(path).unwrap();
}

/// A document alone on its capture keeps the exact WAV alias it always had.
#[tokio::test]
#[serial_test::serial]
async fn a_lone_document_keeps_its_exact_wav_alias() {
    let root = tempfile::tempdir().unwrap();
    let mut recorder = StreamingRecorder::new().unwrap();
    let mut fake = fake(&mut recorder, false);
    start_document(&mut recorder, root.path(), "doc-solo").await;
    fake.started.recv().await.unwrap();
    feed(&recorder, &mut fake, 120, 120).await;
    let (last, mut take) = stop_document(&mut recorder).await;
    assert!(last);
    let (_, wav) = take.finish().await.unwrap();
    let wav = wav.expect("one interval: the document's exact WAV");
    assert_eq!(hound::WavReader::open(&wav).unwrap().duration(), 120);
    assert!(
        take.released_shared_capture().is_none(),
        "the document itself retains the archive"
    );
    assert!(take.take_shared_capture_archive().is_none());
    assert!(take.is_settled());
    assert_eq!(fake.receipts.load(Ordering::SeqCst), 1);
    assert!(recorder.shared_channel_view().is_none());
    std::fs::remove_file(wav).unwrap();
}
