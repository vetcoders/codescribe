//! Sample-clock falsifiers: no device, decoder, model or wall-clock sleeps.
use super::*;
use crate::audio::streaming_recorder::StreamingRecorder;
use crate::pipeline::acoustic_ledger::{MutationReceipt, ObservationIdentity, ObservationProducer};
use std::task::Poll;

const CAPTURE: &str = "agent-channels-falsifier";
const RATE: u64 = 16_000;

/// One seat's receipts, by route: ranged text and ledger mutations,
/// capture-end settlement, live capture state, and the formatter flag.
#[derive(Default)]
struct Sink {
    words: StdMutex<Vec<(u64, u64, String)>>,
    mutations: StdMutex<Vec<String>>,
    settlements: StdMutex<Vec<&'static str>>,
    live: StdMutex<Vec<&'static str>>,
    formatter: StdMutex<Vec<bool>>,
}

impl EventSink for Sink {
    fn on_formatter_pending(&self, pending: bool) {
        self.formatter.lock().unwrap().push(pending);
    }

    fn on_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::DirectTranscript {
                observation, text, ..
            } => self.words.lock().unwrap().push((
                observation.occurrence.sample_start,
                observation.occurrence.sample_end,
                text.clone(),
            )),
            EngineEvent::LedgerMutation { label, .. } => {
                self.mutations.lock().unwrap().push(label.clone());
            }
            EngineEvent::DirectTranscriptEnd { .. } => {
                self.settlements.lock().unwrap().push("transcript-end");
            }
            EngineEvent::VadStart { .. } => self.live.lock().unwrap().push("vad-start"),
            _ => {}
        }
    }
}

impl Sink {
    fn words(&self) -> Vec<String> {
        self.words
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, word)| word.clone())
            .collect()
    }

    fn mutations(&self) -> Vec<String> {
        self.mutations.lock().unwrap().clone()
    }

    fn settlements(&self) -> Vec<&'static str> {
        self.settlements.lock().unwrap().clone()
    }

    fn live(&self) -> Vec<&'static str> {
        self.live.lock().unwrap().clone()
    }

    fn formatter(&self) -> Vec<bool> {
        self.formatter.lock().unwrap().clone()
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

/// Ledger mode: the reducer's receipt for the same physical occurrence a
/// `DirectTranscript` would carry; routing must not depend on the mode.
fn mutation(start: u64, end: u64, label: &str) -> EngineEvent {
    let occurrence = OccurrenceIdentity::new(CAPTURE, 1, start, end);
    EngineEvent::LedgerMutation {
        observation: ObservationIdentity::new(
            ObservationProducer::Whisper,
            start,
            0,
            occurrence.clone(),
        ),
        label: label.into(),
        receipt: MutationReceipt::Insert { occurrence },
    }
}

fn transcript_end() -> EngineEvent {
    EngineEvent::DirectTranscriptEnd {
        session_id: CAPTURE.into(),
        capture_epoch: 1,
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

/// HANDOFF §7: a seat attached or handed over while a peer is still settling
/// inherits the formatter flag as it stands, hears no text from before its
/// sample, and the settling peer keeps hearing its own range and every flag
/// update until the settle future removes it.
#[tokio::test(start_paused = true)]
async fn f15_channel_demux_seats_joining_while_a_peer_settles_inherit_the_flag_without_backfill() {
    let mut recorder = StreamingRecorder::new().unwrap();
    let (a, b, c) = (id(&mut recorder), id(&mut recorder), id(&mut recorder));
    let (sa, sb, sc) = (
        Arc::new(Sink::default()),
        Arc::new(Sink::default()),
        Arc::new(Sink::default()),
    );
    let demux = ChannelDemux::new(CAPTURE.into());
    demux.attach(a, None, sa.clone(), 0);
    demux.on_capture_opened(CAPTURE, 1);
    demux.on_event(&word(20, 40, "first"));
    demux.detach(a, 100);
    let settling = demux.settle(a, CHANNEL_DETACH_SETTLE_BUDGET);
    tokio::pin!(settling);
    assert_eq!(futures_util::poll!(&mut settling), Poll::Pending);

    // A formatter job starts while A settles: A still hears the update and
    // B, attaching afterwards, inherits the flag without any earlier text.
    demux.on_formatter_pending(true);
    demux.attach(b, None, sb.clone(), 100);
    assert_eq!(sa.formatter(), [false, true]);
    assert_eq!(sb.formatter(), [true]);
    demux.on_event(&word(80, 110, "a-tail")); // midpoint 95: A's range
    demux.on_event(&word(120, 140, "b-own"));

    // B's next message while A is still settling: C takes over at 150 and
    // inherits the flag the same way.
    assert_eq!(
        demux.hand_over(b, c, None, sc.clone(), 150),
        Some(DeliverySeat::Primary)
    );
    assert_eq!(sc.formatter(), [true]);
    demux.on_event(&word(60, 90, "a-late")); // midpoint 75: still A's
    demux.on_event(&word(140, 170, "boundary")); // midpoint 155: C's
    demux.on_formatter_pending(false);
    assert_eq!(futures_util::poll!(&mut settling), Poll::Pending);
    tokio::time::advance(Duration::from_secs(11)).await;
    assert_eq!(futures_util::poll!(&mut settling), Poll::Pending);

    // The decoder covers A's detach sample: the settle future removes A.
    covered(&demux, 100);
    assert_eq!(settling.await, DetachSettlement::DecoderPassed);
    demux.on_formatter_pending(true);
    demux.on_event(&word(30, 50, "too-late"));

    assert_eq!(sa.words(), ["first", "a-tail", "a-late"]);
    assert_eq!(sb.words(), ["b-own"]);
    assert_eq!(sc.words(), ["boundary"]);
    assert_eq!(sa.formatter(), [false, true, false]);
    assert_eq!(sb.formatter(), [true, false, true]);
    assert_eq!(sc.formatter(), [true, false, true]);
    assert_eq!(demux.attached_count(), 1);
    {
        let state = demux.lock();
        assert_eq!(state.participants.len(), 2, "B settling, C attached");
        assert_eq!(state.settled_participants.len(), 1);
        assert_eq!(state.settled_participants[0].interval.participant, a);
        assert_eq!(state.settled_participants[0].late_occurrences, 1);
    }
    covered(&demux, 150);
    assert_eq!(
        demux.settle(b, Duration::ZERO).await,
        DetachSettlement::DecoderPassed
    );
    assert_eq!(demux.lock().participants.len(), 1);
}

/// HANDOFF l.41: ledger mode routes by the same midpoint as direct text;
/// capture-end settlement reaches every unsettled seat, a settling one
/// included; live capture state reaches attached seats only.
#[tokio::test]
async fn f16_channel_demux_routes_ledger_mutations_like_text_and_settlement_to_unsettled_seats() {
    let mut recorder = StreamingRecorder::new().unwrap();
    let (a, b) = (id(&mut recorder), id(&mut recorder));
    let (sa, sb) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
    let demux = ChannelDemux::new(CAPTURE.into());
    demux.attach(a, None, sa.clone(), 0);
    demux.on_capture_opened(CAPTURE, 1);
    demux.on_event(&mutation(20, 40, "a-insert"));
    demux.on_event(&word(20, 40, "a-text"));
    demux.attach(b, None, sb.clone(), 100);
    demux.detach(a, 100);
    demux.on_event(&mutation(80, 110, "a-tail")); // midpoint 95: A's range
    demux.on_event(&mutation(120, 140, "b-insert"));
    demux.on_event(&word(120, 140, "b-text"));
    demux.on_event(&EngineEvent::VadStart {
        speech_prob: 0.9,
        ts_ms: 7,
    });
    demux.on_event(&transcript_end());
    covered(&demux, 100);
    assert_eq!(
        demux.settle(a, Duration::ZERO).await,
        DetachSettlement::DecoderPassed
    );
    demux.on_event(&transcript_end());
    demux.on_event(&mutation(30, 50, "late")); // settled A: counted, not delivered

    assert_eq!(sa.mutations(), ["a-insert", "a-tail"]);
    assert_eq!(sa.words(), ["a-text"]);
    assert_eq!(sb.mutations(), ["b-insert"]);
    assert_eq!(sb.words(), ["b-text"]);
    assert!(sa.live().is_empty(), "a detached seat hears no live state");
    assert_eq!(sb.live(), ["vad-start"]);
    assert_eq!(sa.settlements(), ["transcript-end"]);
    assert_eq!(sb.settlements(), ["transcript-end", "transcript-end"]);
    assert_eq!(demux.lock().settled_participants[0].late_occurrences, 1);
}
