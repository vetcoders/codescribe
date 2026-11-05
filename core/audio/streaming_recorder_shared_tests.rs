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
