//! Public PCM-only regression fixtures. No private recording or transcript.
use super::*;

fn state() -> AppleSealState {
    let mut state = AppleSealState::new_for_session(16_000, "seal-recovery".into(), 1);
    state.energy_calibration = Some(EnergyCalibration::new("synthetic", 1.0, 1));
    state
}

fn observe(
    state: &mut AppleSealState,
    tx: &mpsc::UnboundedSender<EngineEvent>,
    start: u64,
    end: u64,
    id: u64,
) -> Option<MutationReceipt> {
    admit_ledger_label(
        state,
        tx,
        LabelAdmission {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                id,
                0,
                OccurrenceIdentity::new("seal-recovery", 1, start, end),
            ),
            label: "Iwo",
            energy: EnergyAdmission::QualifyFinalPassGap,
        },
    )
}

#[test]
fn recovery_retention_head_before_120_seconds_remains_qualifiable() {
    let mut state = state();
    let pcm = vec![0.25; 16_000 * 121];
    state.audio.push(&pcm);
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut wav = hound::WavWriter::create(
        file.path(),
        hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for sample in &pcm {
        wav.write_sample((*sample * i16::MAX as f32) as i16)
            .unwrap();
    }
    wav.finalize().unwrap();
    state.terminal_pcm = Some(
        super::super::live_audio_buffer::FinalizedPcmArchive {
            session_id: state.session_id.clone(),
            capture_epoch: 1,
            sample_rate: 16_000,
            sample_count: pcm.len() as u64,
            path: file.path().into(),
        }
        .load(&state.session_id, 1, 16_000, pcm.len() as u64)
        .unwrap(),
    );
    let recovered = state.window_by_samples(0, 16_000).unwrap();
    assert_eq!(recovered.sample_start, 0);
    assert_eq!(recovered.sample_end, 16_000);
    assert!(
        recovered
            .samples
            .iter()
            .all(|sample| (*sample - 0.25).abs() <= 1.0 / i16::MAX as f32)
    );

    assert!(
        state.audio.window_by_samples(0, 16_000).is_none(),
        "live retention must stay bounded"
    );
    let (tx, _) = mpsc::unbounded_channel();
    assert!(
        observe(&mut state, &tx, 0, 16_000, 1).is_some(),
        "terminal owned PCM must recover the evicted head"
    );
}

#[test]
fn recovery_open_silero_final_waits_for_stable_extent() {
    let mut state = state();
    state.audio.push(&vec![0.25; 32_000]);
    let mut fusion = SileroIngress::new(16_000, state.session_id.clone(), 1);
    fusion
        .ledger_mut()
        .open_or_extend(&state.session_id, 1, 0, 16_000);
    state.fusion = Some(fusion);
    state.fusion_seal_armed = true;
    let (tx, _) = mpsc::unbounded_channel();
    let words = vec![TranscriptSegment {
        text: "Iwo".into(),
        start_ts: 0.1,
        end_ts: 0.8,
    }];
    assert!(seal_sliced_by_silero(&mut state, &tx, &words));
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .rendered_text()
            .is_empty(),
        "open physical occurrence must not acquire a sealed identity"
    );
    state
        .fusion
        .as_mut()
        .unwrap()
        .ledger_mut()
        .open_or_extend(&state.session_id, 1, 0, 32_000);
    state
        .fusion
        .as_mut()
        .unwrap()
        .ledger_mut()
        .close_open(32_000);
    assert!(seal_sliced_by_silero(&mut state, &tx, &words));
    assert_eq!(state.acoustic_ledger.lock().unwrap().rendered_text(), "Iwo");
}

#[test]
fn recovery_equal_words_on_disjoint_pcm_stay_distinct() {
    let mut state = state();
    state.audio.push(&vec![0.25; 32_000]);
    let (tx, _) = mpsc::unbounded_channel();
    assert!(observe(&mut state, &tx, 0, 16_000, 1).is_some());
    assert!(observe(&mut state, &tx, 16_000, 32_000, 2).is_some());
    assert_eq!(
        state.acoustic_ledger.lock().unwrap().rendered_text(),
        "Iwo Iwo"
    );
}

/// Take 9608b50e, 48 kHz. Three closed Silero windows overlap by the live
/// pads (13 824 and 18 432 samples). Each carries its own Apple word on PCM
/// that only that window owns. The middle word has to reach the committed
/// projection; an overlap must not erase it without a conservation refusal.
#[test]
fn overlapping_pad_occurrences_project_each_apple_word() {
    const RATE: u32 = 48_000;
    const END: usize = 4_627_968;
    let mut state = AppleSealState::new_for_session(RATE, "take-9608".into(), 1);
    state.energy_calibration = Some(EnergyCalibration::new("synthetic", 1.0, 1));
    state.audio.push(&vec![0.2; END]);
    let mut fusion = SileroIngress::new(RATE, state.session_id.clone(), 1);
    let session = state.session_id.clone();
    {
        let ledger = fusion.ledger_mut();
        ledger.open_or_extend(&session, 1, 3_454_464, 3_926_016);
        ledger.close_open(3_926_016);
        ledger.open_or_extend(&session, 1, 3_912_192, 4_236_288);
        ledger.close_open(4_236_288);
        ledger.open_or_extend(&session, 1, 4_217_856, 4_627_968);
        ledger.close_open(4_627_968);
    }
    state.fusion = Some(fusion);
    let (tx, _) = mpsc::unbounded_channel();
    let words = [
        TranscriptSegment {
            text: "Leftside".into(),
            start_ts: 3_600_000.0 / RATE as f32,
            end_ts: 3_700_000.0 / RATE as f32,
        },
        TranscriptSegment {
            text: "Middlephrase".into(),
            start_ts: 4_000_000.0 / RATE as f32,
            end_ts: 4_100_000.0 / RATE as f32,
        },
        TranscriptSegment {
            text: "Rightside".into(),
            start_ts: 4_400_000.0 / RATE as f32,
            end_ts: 4_500_000.0 / RATE as f32,
        },
    ];
    assert!(seal_sliced_by_silero(&mut state, &tx, &words));
    let ledger = state.acoustic_ledger.lock().unwrap();
    let rendered = ledger.rendered_text();
    assert!(
        rendered.contains("Middlephrase"),
        "middle Apple word never reached the committed projection: {rendered}"
    );
    assert!(rendered.contains("Leftside"), "{rendered}");
    assert!(rendered.contains("Rightside"), "{rendered}");
    let ranges = ledger
        .occurrences()
        .map(|occurrence| (occurrence.sample_start, occurrence.sample_end))
        .collect::<Vec<_>>();
    assert_eq!(ranges.len(), 3, "{ranges:?}");
    for pair in ranges.windows(2) {
        assert!(
            pair[0].1 <= pair[1].0,
            "committed occurrences still share PCM: {ranges:?}"
        );
    }
    let conservation = SessionConservationReceipt::from_ledger(&ledger, 0, 0, 0, BTreeMap::new());
    assert_eq!(
        conservation.residue(),
        0,
        "admitted minus delivered must equal named refusals: {conservation:?}"
    );
}

#[test]
fn recovery_new_gap_formatter_work_prevents_terminal_seal() {
    let mut state = state();
    state.audio.push(&vec![0.25; 16_000]);
    let (formatter, mut requests) = mpsc::channel(FORMATTER_QUEUE_CAP);
    state.formatter = Some(formatter);
    let (tx, _) = mpsc::unbounded_channel();
    assert!(observe(&mut state, &tx, 0, 16_000, 1).is_some());
    assert!(requests.try_recv().is_ok());
    assert_eq!(state.formatter_awaiting_completion, 1);
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .seal_terminal(&state.session_id, 1)
            .is_err()
    );
}

#[test]
fn recovery_closed_occurrence_submits_owned_tail_job() {
    let mut state = state();
    state.audio.push(&vec![0.25; 32_000]);
    let mut fusion = SileroIngress::new(16_000, state.session_id.clone(), 1);
    fusion
        .ledger_mut()
        .open_or_extend(&state.session_id, 1, 0, 32_000);
    fusion.ledger_mut().close_open(32_000);
    state.fusion = Some(fusion);
    state.fusion_seal_armed = true;
    let (tail, mut jobs) = mpsc::channel(TAIL_PATCH_QUEUE_CAP);
    state.tail_patch = Some(tail);
    let (tx, _) = mpsc::unbounded_channel();
    assert!(seal_sliced_by_silero(
        &mut state,
        &tx,
        &[TranscriptSegment {
            text: "Iwo".into(),
            start_ts: 0.1,
            end_ts: 1.8,
        }]
    ));
    state.flush_layer1_coalesce(&tx);
    let job = jobs
        .try_recv()
        .expect("armed local lane must submit real PCM work");
    assert_eq!(job.audio.len(), 32_000);
    assert_eq!(job.provider_request.identity.range.sample_start, 0);
    assert_eq!(job.provider_request.identity.range.sample_end, 32_000);
    assert_eq!(state.tail_patch_awaiting_completion, 1);
}

#[test]
fn recovery_formatter_created_by_gap_closes_before_coverage_and_terminal_seal() {
    let mut state = state();
    state.audio.push(&vec![0.25; 16_000]);
    let mut fusion = SileroIngress::new(16_000, state.session_id.clone(), 1);
    fusion
        .ledger_mut()
        .open_or_extend(&state.session_id, 1, 0, 16_000);
    fusion.ledger_mut().close_open(16_000);
    state.fusion = Some(fusion);
    let (formatter, mut requests) = mpsc::channel(FORMATTER_QUEUE_CAP);
    state.formatter = Some(formatter);
    let (tx, _) = mpsc::unbounded_channel();
    // The capture energy ladder measured this second, so the coverage question
    // has an authenticated answer instead of an empty set. Silero's crossings
    // are not driven here; ownership windows are no longer a speech measurement.
    let mut capture_level =
        crate::audio::capture_receipt::CaptureLevelAccumulator::bound_to(&state.capture_energy);
    capture_level.push_samples(&vec![0.25f32; 16_000]);
    observe(&mut state, &tx, 0, 16_000, 1).unwrap();
    assert_eq!(
        publish_terminal_coverage(&state, &tx).status,
        SealCoverageStatus::Complete
    );
    assert!(
        state
            .acoustic_ledger
            .lock()
            .unwrap()
            .seal_terminal(&state.session_id, 1)
            .is_err(),
        "coverage is not formatter finality"
    );
    let request = requests.try_recv().unwrap();
    let occurrence = request.occurrence.clone();
    let completion = FormatterCompletion::from_result(
        request,
        AiFormatResult {
            text: "Iwo".into(),
            reasoning_text: None,
            status: AiFormatStatus::Skipped,
        },
    );
    // Model the emitter's synchronous no-change return before its worker ACK.
    {
        let mut ledger = state.acoustic_ledger.lock().unwrap();
        assert!(ledger.note_frontier_return(&occurrence, LedgerObservationProducer::Formatter));
        ledger.seal(&occurrence).unwrap();
    }
    let (ack, done) = std_mpsc::channel();
    ack.send(completion.clone()).unwrap();
    drain_formatter_observers(&mut state, &tx, &done).unwrap();
    assert!(
        !state.complete_formatter(&tx, completion),
        "duplicate completion cannot close twice"
    );
    assert_eq!(
        publish_terminal_coverage(&state, &tx).status,
        SealCoverageStatus::Complete
    );
    state
        .acoustic_ledger
        .lock()
        .unwrap()
        .seal_terminal(&state.session_id, 1)
        .unwrap();
    assert_eq!(state.formatter_awaiting_completion, 0);
}

/// Local acoustic-recovery bench, not a microphone/history/delivery witness.
/// Requires an archived take whose logged device calibration can be recovered.
#[test]
#[ignore = "private local WAV and measured capture calibration required"]
fn private_archive_acoustic_recovery_bench() {
    let path =
        std::path::PathBuf::from(std::env::var("CODESCRIBE_REPLAY_WAV").expect("local WAV path"));
    let session = path.file_stem().unwrap().to_str().unwrap().to_owned();
    let log = std::fs::read_to_string(
        directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .join(".codescribe/logs/codescribe.log"),
    )
    .unwrap();
    let line = log
        .lines()
        .find(|line| {
            line.contains("acoustic admission calibration sealed for session")
                && line.contains(&format!("session={session}"))
        })
        .expect("archived session capture receipt");
    let device = line
        .split("device=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("recorded device identity");
    let reader = hound::WavReader::open(&path).unwrap();
    let rate = reader.spec().sample_rate;
    let count = u64::from(reader.duration());
    let snapshot = crate::config::Config::load_runtime_snapshot_without_keychain().unwrap();
    let calibration = snapshot
        .energy_calibration_for_capture(device, rate)
        .expect("measured profile for actual archived device");
    assert!(
        line.contains(&format!("calibration_version={}", calibration.version)),
        "calibration generation must match the archived capture"
    );
    let owned = super::super::live_audio_buffer::FinalizedPcmArchive {
        session_id: session.clone(),
        capture_epoch: 1,
        sample_rate: rate,
        sample_count: count,
        path,
    }
    .load(&session, 1, rate, count)
    .unwrap();
    let mut state = AppleSealState::new_for_session(rate, session.clone(), 1);
    state.energy_calibration = Some(calibration);
    let mut fusion = SileroIngress::new(rate, session, 1);
    assert!(fusion.vad_available());
    let mut cursor = 0;
    for chunk in owned
        .window(0, count)
        .unwrap()
        .samples
        .chunks((rate / 10) as usize)
    {
        cursor += chunk.len() as u64;
        state.audio.push(chunk);
        fusion.ingest(chunk, cursor);
    }
    fusion.flush(count);
    state.fusion = Some(fusion);
    state.terminal_pcm = Some(owned);
    let (tx, _) = mpsc::unbounded_channel();
    let before = publish_terminal_coverage(&state, &tx);
    let execution = LocalExecutionOwner::default();
    // Repair uses blocking_recv, so only the execution join enters Tokio.
    repair_terminal_seal_coverage(&mut state, &tx, Some("pl"), &execution);
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("local execution join runtime")
        .block_on(execution.close_and_join());
    let after = publish_terminal_coverage(&state, &tx);
    let mut ledger = state.acoustic_ledger.lock().unwrap();
    let terminal = if after.status == SealCoverageStatus::Complete {
        ledger.seal_terminal(&state.session_id, 1).is_ok()
    } else {
        false
    };
    println!(
        "LOCAL_ACOUSTIC_BENCH samples={count} rate={rate} before={}/{} max_gap={} after={}/{} max_gap={} threshold={} terminal={} chars={}",
        before.covered_samples,
        before.speech_samples,
        before.max_uncovered_samples,
        after.covered_samples,
        after.speech_samples,
        after.max_uncovered_samples,
        after.incomplete_threshold_samples,
        terminal,
        ledger.rendered_text().chars().count()
    );
    assert!(
        after.speech_samples > 0,
        "zero-occurrence replay is not evidence"
    );
    assert!(terminal, "real PCM did not achieve terminal coverage");
}

#[tokio::test]
#[ignore = "private archived capture provenance; isolated formatting-off settings required"]
async fn private_archive_live_producer_bench() {
    let path =
        std::path::PathBuf::from(std::env::var("CODESCRIBE_REPLAY_WAV").expect("local WAV path"));
    let session = path.file_stem().unwrap().to_str().unwrap().to_owned();
    let log = std::fs::read_to_string(
        directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .join(".codescribe/logs/codescribe.log"),
    )
    .unwrap();
    let line = log
        .lines()
        .find(|line| {
            line.contains("acoustic admission calibration sealed for session")
                && line.contains(&format!("session={session}"))
        })
        .expect("archived capture receipt");
    let device = line
        .split("device=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_owned();
    let reader = hound::WavReader::open(&path).unwrap();
    let rate = reader.spec().sample_rate;
    let count = u64::from(reader.duration());
    let pcm = reader
        .into_samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / f32::from(i16::MAX))
        .collect::<Vec<_>>();
    let snapshot =
        Arc::new(crate::config::Config::load_runtime_snapshot_without_keychain().unwrap());
    assert_eq!(
        snapshot.formatting_policy(),
        FormattingPolicy::Off,
        "private audio may not reach a remote formatter"
    );
    assert_eq!(
        snapshot.tail_provider(),
        Some(crate::stt::tail_provider::TailProviderId::InProcess)
    );
    assert!(snapshot.seal_lane_armed());
    let calibration = snapshot
        .energy_calibration_for_capture(&device, rate)
        .unwrap();
    assert!(line.contains(&format!("calibration_version={}", calibration.version)));
    let layer1 = snapshot.local_tail_patch_decision();
    assert!(layer1.is_armed());
    let ledger = Arc::new(Mutex::new(AcousticLedger::new()));
    let (archive, terminal_audio) = std_mpsc::channel();
    archive
        .send(Ok(super::super::live_audio_buffer::FinalizedPcmArchive {
            session_id: session.clone(),
            capture_epoch: 1,
            sample_rate: rate,
            sample_count: count,
            path,
        }))
        .unwrap();
    let events = super::super::session::collect_buffered_engine_events_with_config(
        &pcm,
        SessionConfig {
            session_id: session.clone(),
            capture_epoch: 1,
            runtime_settings: snapshot,
            live_formatting_agent: None,
            acoustic_ledger: ledger.clone(),
            sample_rate: rate,
            capture_device_name: Some(device),
            language: Some("pl".into()),
            stream_log_path: None,
            utterance_silence_sec: None,
            capture_turn: crate::audio::streaming_recorder::CaptureTurnIntent::HandsFree,
            layer1,
            lifecycle_events: None,
            terminal_audio: Some(terminal_audio),
        },
    )
    .await
    .unwrap();
    let tail = TailPatchSessionReceipt::from_events(&events).expect("production tail receipt");
    let terminal = events.iter().any(|event| matches!(event, EngineEvent::LedgerSeal { receipt } if !receipt.is_occurrence_seal()));
    let ledger = ledger.lock().unwrap();
    if ledger.latest_seal_coverage().is_none() {
        for event in &events {
            match event {
                EngineEvent::NoSpeech { reason } => {
                    let classification = [
                        "permission",
                        "authorized",
                        "timed out",
                        "timeout",
                        "deadline",
                        "bridge",
                        "EOF",
                        "archive",
                        "mismatch",
                        "formatter",
                        "Speech",
                        "worker",
                    ]
                    .into_iter()
                    .filter(|token| {
                        reason
                            .to_ascii_lowercase()
                            .contains(&token.to_ascii_lowercase())
                    })
                    .collect::<Vec<_>>();
                    println!(
                        "LOCAL_PRODUCER_FAILURE classification={classification:?} reason_chars={}",
                        reason.chars().count()
                    );
                }
                EngineEvent::Warning { code, .. } => println!("LOCAL_PRODUCER_WARNING code={code}"),
                _ => {}
            }
        }
    }
    let coverage = ledger
        .latest_seal_coverage()
        .expect("production coverage receipt");
    println!(
        "LOCAL_PRODUCER_BENCH samples={count} rate={rate} covered={}/{} max_gap={} threshold={} terminal={} armed={} submitted={} chars={}",
        coverage.covered_samples,
        coverage.speech_samples,
        coverage.max_uncovered_samples,
        coverage.incomplete_threshold_samples,
        terminal,
        tail.armed,
        tail.submitted,
        ledger.rendered_text().chars().count()
    );
    assert!(coverage.speech_samples > 0);
    assert!(tail.armed && tail.submitted > 0);
    assert_eq!(coverage.status, SealCoverageStatus::Complete);
    assert!(terminal);
}
