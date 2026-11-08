//! Settlement is decoder progress, independent of acoustic sealing debt.
use super::*;
use crate::audio::channel_demux::{ChannelDemux, DetachSettlement};
use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};
use crate::audio::streaming_recorder::StreamingRecorder;
use std::task::Poll;

const SESSION: &str = "agent-channels-decoder-falsifier";
const RATE: u32 = 1_000;

struct NullSink;
impl EventSink for NullSink {
    fn on_event(&self, _: &EngineEvent) {}
}

fn state() -> AppleSealState {
    let (sender, _receiver) = mpsc::channel(8);
    let mut state = AppleSealState::new_with_tail_patch_for_session(
        RATE,
        SESSION.into(),
        1,
        sender,
        Arc::new(Mutex::new(AcousticLedger::new())),
        None,
    );
    state.capture_turn = CaptureTurnIntent::AgentChannel;
    state.audio.push(&vec![0.0; 20_000]);
    while let Some(range) = state.window_plan.next_due(20_000, false) {
        assert!(state.window_plan.account(&range));
    }
    assert_eq!(state.window_plan.admission_horizon(), 12_000);
    state
}

fn coverage(state: &mut AppleSealState) -> Vec<u64> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    state.publish_decoder_coverage(&tx);
    let mut result = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            EngineEvent::DecoderCovered {
                session_id,
                capture_epoch,
                sample_end,
            } => {
                assert_eq!(session_id, SESSION);
                assert_eq!(capture_epoch, 1);
                result.push(sample_end);
            }
            other => panic!("unexpected coverage event: {other:?}"),
        }
    }
    result
}

#[tokio::test]
async fn f11_channel_decoder_coverage_unsealed_owner_and_uncovered_gap_do_not_hold_detach() {
    let mut state = state();
    let owner = OccurrenceIdentity::new(SESSION, 1, 1_000, 2_000);
    let calibration = EnergyCalibration::new("f11", 1.0, 1);
    let evidence = AcousticEvidence {
        occurrence: owner.clone(),
        duration_ms: 1_000.0,
        energy_integral: 10.0,
        mean_rms_dbfs: -12.0,
        peak_dbfs: -3.0,
        vad_open_sample: Some(1_000),
        vad_close_sample: Some(2_000),
        evidence_calibration_version: calibration.version.clone(),
    };
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .qualify(&evidence, &calibration)
            .is_qualified()
    );
    let mut fusion = SileroIngress::new(RATE, SESSION, 1);
    fusion.ingest(&vec![0.0; 20_000], &vec![0.0; 20_000], 20_000);
    fusion.observe_boundaries(&[
        VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechStart,
            sample: 3_000,
            speech_probability: 0.9,
        },
        VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechEnd,
            sample: 4_000,
            speech_probability: 0.1,
        },
    ]);
    // Open fusion ownership is also permanent debt, not in-flight decoding.
    fusion.ledger_mut().open_or_extend(SESSION, 1, 0, 20_000);
    state.fusion = Some(fusion);
    let speech = observed_silero(&state).unwrap();
    assert_eq!(
        uncovered_gap_ranges(
            &state.acoustic_ledger.lock().unwrap(),
            SESSION,
            1,
            &speech,
            0
        )
        .iter()
        .map(|r| (r.sample_start, r.sample_end))
        .collect::<Vec<_>>(),
        [(2_936, 4_064)]
    );
    let mut recorder = StreamingRecorder::new().unwrap();
    let a = recorder.register_channel_feed().id;
    let b = recorder.register_channel_feed().id;
    let demux = ChannelDemux::new(SESSION.into());
    demux.attach(a, None, Arc::new(NullSink), 0);
    demux.attach(b, None, Arc::new(NullSink), 4_000);
    demux.on_capture_opened(SESSION, 1);
    demux.detach(a, 7_000);
    let settling = demux.settle(a, Duration::ZERO);
    tokio::pin!(settling);
    assert_eq!(futures_util::poll!(&mut settling), Poll::Pending);
    let ends = coverage(&mut state);
    assert_eq!(ends, [12_000]);
    for end in ends {
        demux.on_event(&EngineEvent::DecoderCovered {
            session_id: SESSION.into(),
            capture_epoch: 1,
            sample_end: end,
        });
    }
    assert_eq!(
        futures_util::poll!(&mut settling),
        Poll::Ready(DetachSettlement::DecoderPassed)
    );
    assert_eq!(demux.attached_count(), 1);
    assert!(!state.acoustic_ledger.lock().unwrap().is_sealed(&owner));
    assert!(!state.capture_stopping);
}

#[test]
fn f14_channel_decoder_coverage_silence_and_missing_silero_advance() {
    let mut missing = state();
    assert!(observed_silero(&missing).is_none());
    assert_eq!(coverage(&mut missing), [12_000]);
    assert_eq!(coverage(&mut missing), Vec::<u64>::new());
    let mut silent = state();
    let mut fusion = SileroIngress::new(RATE, SESSION, 1);
    fusion.ingest(&vec![0.0; 20_000], &vec![0.0; 20_000], 20_000);
    silent.fusion = Some(fusion);
    assert_eq!(observed_silero(&silent).unwrap().ranges().len(), 0);
    assert_eq!(coverage(&mut silent), [12_000]);
}

#[test]
fn f14_channel_decoder_coverage_older_inflight_window_blocks_newer_return() {
    let mut state = state();
    let job = |seq, start, end| TailPatchInFlight {
        submission_sequence: seq,
        utterance_id: seq,
        request_identity: TailRequestIdentity {
            request_id: seq,
            range: TailSampleRange {
                session: SESSION.into(),
                capture_epoch: 1,
                sample_start: start,
                sample_end: end,
            },
        },
        admit_sample_start: start,
        admit_sample_end: end,
        member_occurrences: Vec::new(),
    };
    state
        .refinement_submitted
        .insert((1, 1, 0, 0), job(1, 0, 9_000));
    state
        .refinement_submitted
        .insert((2, 2, 3_000, 0), job(2, 3_000, 12_000));
    assert_eq!(coverage(&mut state), Vec::<u64>::new());
    state
        .refinement_submitted
        .remove(&(2, 2, 3_000, 0))
        .unwrap();
    assert_eq!(coverage(&mut state), Vec::<u64>::new());
    state.refinement_submitted.remove(&(1, 1, 0, 0)).unwrap();
    assert_eq!(coverage(&mut state), [12_000]);
}

#[test]
fn f14_channel_decoder_coverage_dictation_witness_holds_until_return_or_existing_budget() {
    let mut state = state();
    state.dictation_witness_inflight = true;
    state.dictation_witness_coverage_hold = Some((2_000, Instant::now()));
    assert_eq!(coverage(&mut state), [2_000]);
    state.dictation_witness_inflight = false;
    state.dictation_witness_coverage_hold = None;
    assert_eq!(coverage(&mut state), [12_000]);
    let mut expired = super::channel_decoder_coverage_tests::state();
    expired.dictation_witness_inflight = true;
    expired.dictation_witness_coverage_hold =
        Some((2_000, Instant::now() - DICTATION_WITNESS_REQUEST_TIMEOUT));
    assert_eq!(coverage(&mut expired), [12_000]);
}
