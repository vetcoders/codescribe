//! BlackHole loopback proof on real operator session audio.

#![cfg(target_os = "macos")]

use std::path::Path;
use std::sync::{Arc, Mutex};

use codescribe_core::audio::streaming_recorder::StreamingRecorder;
use codescribe_core::pipeline::contracts::EngineEvent;

struct CollectSink(Arc<Mutex<Vec<EngineEvent>>>);

impl codescribe_core::pipeline::contracts::EventSink for CollectSink {
    fn on_event(&self, event: &EngineEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn blackhole_capture_gate(value: Option<&str>) -> anyhow::Result<()> {
    anyhow::ensure!(
        value == Some("1"),
        "live BlackHole proof requires CODESCRIBE_E2E_CAPTURE_VIA_DEVICE=1 before capture"
    );
    Ok(())
}

#[test]
fn test_blackhole_capture_gate_requires_explicit_opt_in() {
    for value in [None, Some(""), Some("0"), Some("false"), Some("yes")] {
        assert!(blackhole_capture_gate(value).is_err());
    }
    assert!(blackhole_capture_gate(Some("1")).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live BlackHole proof: requires CODESCRIBE_E2E_CAPTURE_VIA_DEVICE=1"]
async fn blackhole_session_proof() {
    blackhole_capture_gate(
        std::env::var("CODESCRIBE_E2E_CAPTURE_VIA_DEVICE")
            .ok()
            .as_deref(),
    )
    .expect("capture opt-in");
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .init();

    let session_wav = std::env::var("PROOF_WAV").unwrap_or_else(|_| {
        "/Users/maciejgad/.codescribe/sessions/15e7b236-94ef-4d60-a388-3870d1aa8923.wav".to_string()
    });
    let wav_path = Path::new(&session_wav);
    assert!(wav_path.exists(), "WAV file does not exist: {session_wav}");

    let bridge_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/release/codescribe-stt-bridge")
        .to_string_lossy()
        .into_owned();
    unsafe {
        std::env::set_var("AUDIO_INPUT_DEVICE", "BlackHole 2ch");
        std::env::set_var("CODESCRIBE_STT_ENGINE", "apple");
        std::env::set_var("CODESCRIBE_APPLE_STT_BRIDGE", bridge_path);
        std::env::set_var("CODESCRIBE_BRIDGE_DISCLAIM", "1");
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(CollectSink(events.clone()));

    eprintln!("==> Initializing StreamingRecorder on BlackHole 2ch...");
    let mut recorder = StreamingRecorder::new().expect("recorder init");
    recorder.set_event_sink(Some(sink.clone()));

    let runtime_settings = Arc::new(
        codescribe_core::config::Config::load_runtime_snapshot_without_keychain()
            .expect("load runtime snapshot"),
    );
    let session_id = uuid::Uuid::new_v4().to_string();
    recorder.bind_session_authority(session_id, runtime_settings);

    eprintln!("==> Starting event session (pl)...");
    recorder
        .start_event_session(Some("pl".to_string()))
        .await
        .expect("start capture session");

    // Give CoreAudio ~500ms to open capture stream before playback starts
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    eprintln!("==> Playing {session_wav} into BlackHole 2ch...");
    let player_script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/audio-play-to-device.swift");
    let player_status = std::process::Command::new("swift")
        .arg(&player_script)
        .arg("BlackHole 2ch")
        .arg(wav_path)
        .status();
    eprintln!("==> Player result: {player_status:?}");

    // Tail drain
    eprintln!("==> Waiting 3s for pipeline drain...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    eprintln!("==> Calling recorder.stop().await...");
    let stop_result = recorder.stop().await;

    // Release this test's capture before reporting a playback failure. Speech
    // heard from another source cannot turn failed WAV playback into proof.
    let status = player_status.expect("run audio-play-to-device");
    assert!(status.success(), "audio-play-to-device failed: {status}");

    eprintln!("==================================================");
    eprintln!("STOP RESULT:");
    match &stop_result {
        Ok((transcript, audio_path)) => {
            eprintln!("STATUS: OK (Clean stop)");
            eprintln!("TRANSCRIPT: '{transcript}'");
            eprintln!("AUDIO PATH: {audio_path:?}");
        }
        Err(err) => {
            eprintln!("STATUS: REFUSED / ERROR: {err:#}");
            if let Some(refusal) = err
                .downcast_ref::<codescribe_core::audio::streaming_recorder::TerminalSealRefused>(
            ) {
                eprintln!("TerminalSealRefused details:");
                eprintln!("  reason: {}", refusal.finality.reason().as_str());
                eprintln!(
                    "  session: {} epoch: {}",
                    refusal.finality.session_id(),
                    refusal.finality.capture_epoch()
                );
                eprintln!("  measured_coverage: {:?}", refusal.finality.coverage());
                eprintln!("  committed_text: '{}'", refusal.committed_text);
                eprintln!("  audio_path: {:?}", refusal.audio_path);
            }
        }
    }
    eprintln!("==================================================");

    let captured_events = events.lock().unwrap().clone();
    eprintln!("Total EngineEvents emitted: {}", captured_events.len());
    for (i, ev) in captured_events.iter().enumerate() {
        eprintln!("  [{i}] {ev:?}");
    }
    eprintln!("==================================================");

    // A proof that accepts silence proves nothing (Founder 2026-09-09: "to są
    // testy, a nie byle było"). The loopback must deliver real speech into the
    // capture, and the capture must come back with words.
    let digital_silence = captured_events.iter().any(|ev| {
        matches!(ev, EngineEvent::Warning { code, message }
            if code == "capture_level_low" && message.contains("all_audio_median_db=-inf"))
    });
    assert!(
        !digital_silence,
        "BlackHole capture was digital zero: the player never delivered the WAV into the device"
    );
    let speech_samples = captured_events.iter().find_map(|ev| match ev {
        EngineEvent::SealCoverage { receipt, .. } => Some(receipt.speech_samples),
        _ => None,
    });
    assert!(
        speech_samples.is_some_and(|n| n > 0),
        "no speech samples reached the ledger: {speech_samples:?}"
    );
    let transcript = match &stop_result {
        Ok((text, _)) => text.clone(),
        Err(err) => err
            .downcast_ref::<codescribe_core::audio::streaming_recorder::TerminalSealRefused>()
            .map(|refusal| refusal.committed_text.clone())
            .unwrap_or_default(),
    };
    assert!(
        !transcript.trim().is_empty(),
        "real speech played through BlackHole produced no transcript"
    );
}
