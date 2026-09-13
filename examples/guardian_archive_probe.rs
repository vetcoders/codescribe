//! Saved private PCM diagnostic: no microphone, bus, clipboard or settings writes.
//! Uses historical capture provenance, not an invented device calibration.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, ensure};
use codescribe_core::audio::streaming_recorder::CaptureTurnIntent;
use codescribe_core::config::Config;
use codescribe_core::pipeline::acoustic_ledger::AcousticLedger;
use codescribe_core::pipeline::contracts::EngineEvent;
use codescribe_core::pipeline::streaming::{
    SessionConfig, collect_buffered_engine_events_with_config,
};
use codescribe_core::stt::tail_provider::TailProviderId;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let mut args = std::env::args_os().skip(1);
    let wav = PathBuf::from(args.next().context("WAV path required")?);
    let original_log = PathBuf::from(args.next().context("original app log required")?);
    let output = PathBuf::from(args.next().context("new report path required")?);
    ensure!(!output.exists(), "report already exists");
    let session = wav
        .file_stem()
        .and_then(|s| s.to_str())
        .context("session stem")?;
    let log = std::fs::read_to_string(&original_log)?;
    let provenance = log
        .lines()
        .find(|line| {
            line.contains("acoustic admission calibration sealed for session")
                && line.contains(&format!("session={session} "))
        })
        .context("no exact original capture receipt")?;
    let device = provenance
        .split("device=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .context("device provenance missing")?;
    let reader = hound::WavReader::open(&wav)?;
    let spec = reader.spec();
    ensure!(
        spec.channels == 1
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "requires original mono PCM16 archive"
    );
    ensure!(
        provenance.contains(&format!("sample_rate={} ", spec.sample_rate)),
        "capture rate mismatch"
    );
    let samples = reader
        .into_samples::<i16>()
        .map(|sample| sample.map(|value| f32::from(value) / 32768.0))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let snapshot = Arc::new(
        Config::load_runtime_snapshot_without_keychain()
            .map_err(|_| anyhow::anyhow!("runtime snapshot refused"))?,
    );
    ensure!(
        snapshot.tail_provider() == Some(TailProviderId::InProcess),
        "diagnostic requires local Whisper"
    );
    ensure!(snapshot.seal_lane_armed(), "Silero seal lane unarmed");
    let calibration = snapshot
        .energy_calibration_for_capture(device, spec.sample_rate)
        .map_err(|_| anyhow::anyhow!("capture calibration unavailable"))?;
    ensure!(
        provenance.contains(&format!("calibration_version={} ", calibration.version)),
        "calibration changed since original capture"
    );
    let calibration_version = calibration.version.clone();
    let layer1 = snapshot.local_tail_patch_decision();
    ensure!(layer1.is_armed(), "local refinement unarmed");
    let ledger = Arc::new(Mutex::new(AcousticLedger::new()));
    eprintln!(
        "guardian archive diagnostic: {} samples at {} Hz; real PCM, SingleTurn, local Whisper",
        samples.len(),
        spec.sample_rate
    );
    let events = collect_buffered_engine_events_with_config(
        &samples,
        SessionConfig {
            session_id: session.to_owned(),
            capture_epoch: 1,
            runtime_settings: snapshot,
            acoustic_ledger: ledger.clone(),
            sample_rate: spec.sample_rate,
            capture_device_name: Some(device.to_owned()),
            language: Some("pl".into()),
            stream_log_path: None,
            utterance_silence_sec: None,
            capture_turn: CaptureTurnIntent::SingleTurn,
            layer1,
            lifecycle_events: None,
            terminal_audio: None,
        },
    )
    .await?;
    let mut timeline = Vec::new();
    let mut warning_codes = BTreeMap::<String, usize>::new();
    let mut terminal = false;
    for (ordinal, event) in events.iter().enumerate() {
        match event {
            EngineEvent::SpeechIntegrity { evidence } => {
                timeline.push(json!({"ordinal":ordinal,"kind":"integrity","evidence":evidence}))
            }
            EngineEvent::LedgerMutation {
                observation,
                label,
                receipt,
            } => timeline.push(json!({
                "ordinal":ordinal,"kind":"mutation","producer":observation.producer.as_str(),
                "start":observation.occurrence.sample_start,"end":observation.occurrence.sample_end,
                "label_chars":label.chars().count(),"grants_mutation":receipt.grants_mutation(),
            })),
            EngineEvent::LedgerSeal { receipt } => {
                terminal |= !receipt.is_occurrence_seal();
                timeline.push(json!({"ordinal":ordinal,"kind":"seal","terminal":!receipt.is_occurrence_seal()}));
            }
            EngineEvent::Warning { code, message } => {
                *warning_codes.entry(code.clone()).or_default() += 1;
                timeline.push(
                    json!({"ordinal":ordinal,"kind":"warning","code":code,"message":message}),
                );
            }
            EngineEvent::NoSpeech { reason } => {
                timeline.push(json!({"ordinal":ordinal,"kind":"no_speech","reason":reason}));
            }
            _ => {}
        }
    }
    let ledger = ledger.lock().unwrap();
    let coverage = ledger.latest_seal_coverage().map(|receipt| {
        json!({
            "status":format!("{:?}",receipt.status),"speech_samples":receipt.speech_samples,
            "covered_samples":receipt.covered_samples,"ratio":receipt.coverage_ratio(),
            "max_uncovered_samples":receipt.max_uncovered_samples,
            "producer":receipt.speech_producer,"availability":receipt.availability,
        })
    });
    let report = json!({
        "input":wav,"original_log":original_log,"capture_receipt_timestamp":provenance.split_whitespace().next(),
        "device":device,"calibration":calibration_version,"sample_rate":spec.sample_rate,"samples":samples.len(),
        "scope":"real saved PCM; SingleTurn diagnostic, not live microphone/UI/delivery",
        "terminal_archive_supplied":false,"terminal_seal":terminal,"coverage":coverage,
        "qualified_occurrences":ledger.qualified_occurrences().count(),
        "committed_occurrences":ledger.len(),"pending_recoveries":ledger.pending_text_recoveries(session,1).len(),
        "warnings":warning_codes,"event_count":events.len(),"timeline":timeline,
    });
    std::fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", output.display());
    Ok(())
}
