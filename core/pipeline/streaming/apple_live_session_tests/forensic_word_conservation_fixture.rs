// Controlled provider responses over measured synthetic capture PCM.
// This exercises scheduling and admission, not recognizer accuracy.
use super::*;
use crate::pipeline::acoustic_ledger::{EnergyCalibration, OccurrenceIdentity};
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderEvidence, TailProviderId,
    TailProviderPayload, TailTimingQuality,
};
use tokio::sync::mpsc;
const RATE: u32 = 16_000;
struct Lane {
    state: AppleSealState,
    tx: mpsc::UnboundedSender<EngineEvent>,
    rx: mpsc::UnboundedReceiver<EngineEvent>,
    tail_rx: mpsc::Receiver<TailPatchRequest>,
}
fn open(session: &str) -> Lane {
    let (tx, rx) = mpsc::unbounded_channel();
    let (tail_tx, tail_rx) = mpsc::channel(8);
    let mut state = AppleSealState::new_for_session(RATE, session.to_string(), 1);
    state.tail_patch = Some(tail_tx);
    Lane {
        state,
        tx,
        rx,
        tail_rx,
    }
}

fn segment(session: &str, text: &str, start: u64, end: u64) -> TimedTailSegment {
    TimedTailSegment {
        confidence: None,
        grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
        text: text.to_string(),
        range: crate::stt::tail_provider::TailSampleRange {
            session: session.to_string(),
            capture_epoch: 1,
            sample_start: start,
            sample_end: end,
        },
    }
}

fn word_pin(session: &str, text: &str, start: u64, end: u64) -> TimedTailSegment {
    let mut pin = segment(session, text, start, end);
    pin.grain = crate::stt::tail_provider::TailSegmentGrain::Word;
    pin
}

fn completion(request: &TailPatchRequest, segments: Vec<TimedTailSegment>) -> TailPatchCompletion {
    let text = segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let segment_grain = if !segments.is_empty()
        && segments
            .iter()
            .all(|segment| segment.grain == crate::stt::tail_provider::TailSegmentGrain::Word)
    {
        crate::stt::tail_provider::TailSegmentGrain::Word
    } else {
        crate::stt::tail_provider::TailSegmentGrain::Phrase
    };
    TailPatchCompletion {
        submission_sequence: request.submission_sequence,
        utterance_id: request.utterance_id,
        request_identity: Some(request.provider_request.identity.clone()),
        payload: Some(TailProviderPayload {
            identity: request.provider_request.identity.clone(),
            text,
            segments,
            avg_logprob: Some(-0.2),
            compression_ratio: Some(1.1),
            provider_id: TailProviderId::Fake,
            elapsed_ms: 1,
            evidence: TailProviderEvidence {
                segment_grain,
                source: TailEvidenceSource::Whisper,
                revision: Some("forensic-word-grain".into()),
                stability: TailEvidenceStability::Final,
                timing_quality: TailTimingQuality::ExactSampleRange,
                avg_logprob: Some(-0.2),
                vad_selected_samples: None,
            },
        }),
        member_occurrences: request.member_occurrences.clone(),
    }
}

fn drain(rx: &mut mpsc::UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn take_requests(rx: &mut mpsc::Receiver<TailPatchRequest>) -> Vec<TailPatchRequest> {
    let mut requests = Vec::new();
    while let Ok(request) = rx.try_recv() {
        requests.push(request);
    }
    requests
}

fn record_energy(lane: &Lane, blocks: &[Vec<f32>]) {
    let mut writer = CaptureLevelAccumulator::bound_to(&lane.state.capture_energy);
    for block in blocks {
        writer.push_samples(block);
    }
}

/// Silero hears the capture up to `speech_end` as one speech range: its
/// merge gap joins the pauses between the words. Whisper words enter only
/// over measured speech; unmeasured words stay visible evidence without
/// insertion (96b602fa).
fn record_silero(lane: &mut Lane, observed_end: u64, speech_end: u64) {
    use crate::audio::chunker::{VadBoundaryEvidence, VadBoundaryKind};

    let mut fusion = SileroIngress::new(RATE, lane.state.session_id.clone(), 1);
    assert!(
        fusion.vad_available(),
        "the fixture needs a measuring Silero"
    );
    fusion.note_observed_pcm(observed_end, observed_end);
    fusion.observe_boundaries(&[
        VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechStart,
            sample: 0,
            speech_probability: 0.9,
        },
        VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechEnd,
            sample: speech_end,
            speech_probability: 0.1,
        },
    ]);
    lane.state.fusion = Some(fusion);
}

pub fn capture_trace(
    fragmented_ingress: bool,
    contracted: bool,
) -> Vec<(
    crate::pipeline::acoustic_ledger::AcousticLedger,
    Vec<EngineEvent>,
)> {
    let session = match (fragmented_ingress, contracted) {
        (false, false) => "B45-coarse-windows",
        (true, false) => "B45-coarse-fragmented",
        (false, true) => "B45-pinned-windows",
        (true, true) => "B45-pinned-fragmented",
    };
    // Five physical "Iwo" after the second grid window's start (3 s), so
    // both decoder windows contain them: 206 ms each, which meets the
    // two-syllable repeat floor, with sub-floor PCM between them so every
    // repeat has its own voiced segment (96b602fa).
    const SECOND_WINDOW_START: u64 = 48_000;
    const WORDS: [(u64, u64); 5] = [
        (4_000, 7_300),
        (9_400, 12_700),
        (14_800, 18_100),
        (20_200, 23_500),
        (25_600, 28_900),
    ];
    let mut lane = open(session);
    let mut pcm = vec![0.2_f32; 200_000];
    for pair in WORDS.windows(2) {
        let start = (SECOND_WINDOW_START + pair[0].1) as usize;
        let end = (SECOND_WINDOW_START + pair[1].0) as usize;
        pcm[start..end].fill(0.001);
    }
    // The burst ends with the last word, inside both windows: speech that
    // runs on past a window end leaves its words to the next window.
    let speech_end = SECOND_WINDOW_START + WORDS[4].1;
    pcm[speech_end as usize..].fill(0.001);
    if fragmented_ingress {
        for block in pcm.chunks(320) {
            lane.state.audio.push(block);
            lane.state.pump_capture_windows(&lane.tx);
        }
    } else {
        lane.state.audio.push(&pcm);
        lane.state.pump_capture_windows(&lane.tx);
    }
    record_energy(
        &lane,
        &pcm.chunks(320).map(<[f32]>::to_vec).collect::<Vec<_>>(),
    );
    let owner = OccurrenceIdentity::new(session, 1, 0, 200_000);
    lane.state.energy_calibration = Some(EnergyCalibration {
        version: "B45-measured-capture".into(),
        min_energy_integral: 1.0,
        min_valley_samples: 1,
    });
    assert!(qualify_owned_occurrence(&lane.state, &owner));
    assert!(
        lane.state
            .acoustic_ledger
            .lock()
            .expect("recovery obligation")
            .require_text_recovery(&owner)
    );
    lane.state.pending_events.insert(
        1,
        PendingAppleSeal {
            occurrence: owner.clone(),
            raw_text: String::new(),
            layer1_baseline: String::new(),
            start_ts: 0.0,
            end_ts: 12.5,
            segments: Vec::new(),
        },
    );
    assert!(lane.state.register_whisper_owner(&lane.tx, 1, &owner));
    lane.state.capture_stopping = true;
    lane.state.pump_capture_windows(&lane.tx);
    let requests = take_requests(&mut lane.tail_rx);
    assert_eq!(
        requests.len(),
        3,
        "12.5s capture has two full grid windows and one EOF residual"
    );

    for request in &requests {
        let frame = &request.provider_request.identity.range;
        assert_eq!(
            request.audio,
            pcm[frame.sample_start as usize..frame.sample_end as usize]
        );
        request
            .provider_request
            .validate_pcm(&request.audio)
            .expect("actual capture payload");
    }
    let frame = &requests[0].provider_request.identity.range;
    assert!(frame.sample_start <= 4_000 && frame.sample_end >= 60_000);
    let mut snapshots = vec![(
        lane.state
            .acoustic_ledger
            .lock()
            .expect("actual ledger")
            .clone(),
        drain(&mut lane.rx),
    )];
    let first_frame = &requests[0].provider_request.identity.range;
    let second_frame = &requests[1].provider_request.identity.range;
    let base = first_frame.sample_start.max(second_frame.sample_start);
    let shared_end = first_frame.sample_end.min(second_frame.sample_end);
    assert!(
        shared_end >= base + 29_000,
        "both actual decoder windows must contain all five physical Words"
    );
    assert_eq!(base, SECOND_WINDOW_START, "the 9 s grid steps by 3 s");
    record_silero(&mut lane, pcm.len() as u64, speech_end);
    lane.state.existence_floor_rms = Some(0.01);
    let first = WORDS
        .iter()
        .map(|&(start, end)| word_pin(session, "Iwo", base + start, base + end))
        .collect::<Vec<_>>();
    lane.state
        .complete_whisper_window(&lane.tx, completion(&requests[0], first.clone()), 12.5);
    assert_eq!(
        lane.state
            .acoustic_ledger
            .lock()
            .expect("first window")
            .text_of(&owner),
        Some("Iwo Iwo Iwo Iwo Iwo")
    );
    snapshots.push((
        lane.state
            .acoustic_ledger
            .lock()
            .expect("first ledger")
            .clone(),
        drain(&mut lane.rx),
    ));
    let second = if contracted {
        vec![
            word_pin(session, "Iwo", base + 4_000, base + 14_000),
            word_pin(session, "Iwo", base + 14_000, base + 29_000),
        ]
    } else {
        first
    };
    let frame = &requests[1].provider_request.identity.range;
    assert!(
        second
            .iter()
            .all(|pin| pin.range.sample_start >= frame.sample_start
                && pin.range.sample_end <= frame.sample_end)
    );
    lane.state
        .complete_whisper_window(&lane.tx, completion(&requests[1], second), 12.5);
    snapshots.push((
        lane.state
            .acoustic_ledger
            .lock()
            .expect("second ledger")
            .clone(),
        drain(&mut lane.rx),
    ));
    lane.state
        .return_outstanding_whisper_without_label(&lane.tx);
    snapshots.push((
        lane.state
            .acoustic_ledger
            .lock()
            .expect("actual ledger")
            .clone(),
        drain(&mut lane.rx),
    ));
    snapshots
}
