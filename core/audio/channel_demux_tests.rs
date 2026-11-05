//! Sample-clock falsifiers: no device, decoder, model or wall-clock sleeps.
use super::*;
use crate::audio::streaming_recorder::StreamingRecorder;
use crate::pipeline::acoustic_ledger::{ObservationIdentity, ObservationProducer};
use std::task::Poll;

const CAPTURE: &str = "agent-channels-falsifier";
const RATE: u64 = 16_000;

#[derive(Default)]
struct Sink(StdMutex<Vec<(u64, u64, String)>>);

impl EventSink for Sink {
    fn on_event(&self, event: &EngineEvent) {
        if let EngineEvent::DirectTranscript {
            observation, text, ..
        } = event
        {
            self.0.lock().unwrap().push((
                observation.occurrence.sample_start,
                observation.occurrence.sample_end,
                text.clone(),
            ));
        }
    }
}

impl Sink {
    fn words(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, word)| word.clone())
            .collect()
    }
}

fn id(recorder: &mut StreamingRecorder) -> CaptureSubscriberId {
    recorder.register_channel_feed().id
}

fn word(start: u64, end: u64, text: &str) -> EngineEvent {
    EngineEvent::DirectTranscript {
        observation: ObservationIdentity::new(
            ObservationProducer::Whisper,
            start,
            0,
            OccurrenceIdentity::new(CAPTURE, 1, start, end),
        ),
        text: text.into(),
        final_chunk: true,
    }
}

fn covered(demux: &ChannelDemux, end: u64) {
    demux.on_event(&EngineEvent::DecoderCovered {
        session_id: CAPTURE.into(),
        capture_epoch: 1,
        sample_end: end,
    });
}

#[tokio::test]
async fn f1_channel_demux_routes_exact_half_open_intervals_by_midpoint() {
    let mut recorder = StreamingRecorder::new().unwrap();
    let (a, b) = (id(&mut recorder), id(&mut recorder));
    let (sa, sb) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
    let demux = ChannelDemux::new(CAPTURE.into());
    demux.attach(a, None, sa.clone(), 0);
    demux.on_capture_opened(CAPTURE, 1);
    demux.on_event(&word(RATE, 2 * RATE, "before"));
    demux.attach(b, None, sb.clone(), 4 * RATE);
    demux.on_event(&word(39 * RATE / 10, 42 * RATE / 10, "join"));
    demux.on_event(&word(5 * RATE, 6 * RATE, "overlap"));
    demux.detach(a, 7 * RATE);
    // Returned after detach: its midpoint (6.95 s) is still A's.
    demux.on_event(&word(68 * RATE / 10, 71 * RATE / 10, "tail"));
    // Midpoint is exactly 7 s: the right side of A's half-open boundary.
    demux.on_event(&word(69 * RATE / 10, 71 * RATE / 10, "leave"));
    demux.on_event(&word(8 * RATE, 9 * RATE, "after"));
    demux.detach(b, 10 * RATE);
    covered(&demux, 10 * RATE);
    assert_eq!(
        demux.settle(a, Duration::ZERO).await,
        DetachSettlement::DecoderPassed
    );
    assert_eq!(
        demux.settle(b, Duration::ZERO).await,
        DetachSettlement::DecoderPassed
    );
    assert_eq!(sa.words(), ["before", "join", "overlap", "tail"]);
    assert_eq!(sb.words(), ["join", "overlap", "tail", "leave", "after"]);
    assert_eq!(
        demux
            .intervals()
            .iter()
            .map(|i| (i.start_sample, i.end_sample))
            .collect::<Vec<_>>(),
        [(0, Some(7 * RATE)), (4 * RATE, Some(10 * RATE))]
    );
}

#[tokio::test]
async fn f8_channel_demux_reused_digit_keeps_distinct_lease_and_interval() {
    let mut recorder = StreamingRecorder::new().unwrap();
    let (old, new) = (id(&mut recorder), id(&mut recorder));
    let (so, sn) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
    let owner = |session: &str| ChannelParticipantOwner {
        provider: "codex".into(),
        provider_session_id: session.into(),
        lease_id: format!("lease-{session}"),
        channel: "2".into(),
    };
    let demux = ChannelDemux::new(CAPTURE.into());
    demux.attach(old, Some(owner("old")), so.clone(), 0);
    demux.on_capture_opened(CAPTURE, 1);
    demux.detach(old, 100);
    demux.attach(new, Some(owner("new")), sn.clone(), 100);
    demux.on_event(&word(50, 90, "old"));
    demux.on_event(&word(100, 140, "new"));
    covered(&demux, 100);
    assert_eq!(
        demux.settle(old, Duration::ZERO).await,
        DetachSettlement::DecoderPassed
    );
    demux.on_event(&word(60, 80, "too-late"));
    assert_eq!(so.words(), ["old"]);
    assert_eq!(sn.words(), ["new"]);
    let intervals = demux.intervals();
    assert_eq!(intervals[0].owner, Some(owner("old")));
    assert_eq!(intervals[1].owner, Some(owner("new")));
    assert_ne!(intervals[0].participant, intervals[1].participant);
    assert_eq!(demux.lock().settled_participants[0].late_occurrences, 1);
}

#[tokio::test(start_paused = true)]
async fn f14_channel_demux_budget_is_diagnostic_and_late_text_never_backfills() {
    let mut recorder = StreamingRecorder::new().unwrap();
    let (a, b) = (id(&mut recorder), id(&mut recorder));
    let (sa, sb) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
    let demux = ChannelDemux::new(CAPTURE.into());
    demux.attach(a, None, sa.clone(), 0);
    demux.on_capture_opened(CAPTURE, 1);
    let initial = word(20, 40, "first");
    demux.on_event(&initial);
    demux.detach(a, 100);
    demux.attach(b, None, sb.clone(), 100);
    let settling = demux.settle(a, CHANNEL_DETACH_SETTLE_BUDGET);
    tokio::pin!(settling);
    assert_eq!(futures_util::poll!(&mut settling), Poll::Pending);
    tokio::time::advance(Duration::from_secs(11)).await;
    assert_eq!(futures_util::poll!(&mut settling), Poll::Pending);
    demux.on_event(&word(80, 110, "delayed-tail")); // midpoint 95, before detach
    covered(&demux, 99);
    assert_eq!(futures_util::poll!(&mut settling), Poll::Pending);
    covered(&demux, 100);
    assert_eq!(settling.await, DetachSettlement::DecoderPassed);
    // A refinement of an already seen identity counts once after settlement.
    let refined = word(20, 40, "refined");
    demux.on_event(&refined);
    demux.on_event(&refined);
    demux.on_event(&word(60, 80, "late-new-identity"));
    demux.on_event(&word(120, 140, "companion"));
    assert_eq!(sa.words(), ["first", "delayed-tail"]);
    assert_eq!(sb.words(), ["companion"]);
    let state = demux.lock();
    assert_eq!(state.participants.len(), 1);
    assert_eq!(state.settled_participants.len(), 1);
    assert_eq!(state.settled_participants[0].late_occurrences, 2);
    assert_eq!(
        state.settled_participants[0].outcome,
        DetachSettlement::DecoderPassed
    );
}
