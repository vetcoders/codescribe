//! Measure BlackHole through the existing guided-calibration controller.
//! Explicit opt-in: CODESCRIBE_E2E_CAPTURE_VIA_DEVICE=1 AUDIO_INPUT_DEVICE='BlackHole 2ch'.
//! Arguments: original WAV, compiled audio-play-to-device executable.
//! Persists a real profile through the controller; does not change OS devices.
use std::{path::PathBuf, process::Command, time::Duration};

use anyhow::{Context, Result, ensure};
use codescribe::controller::RecordingController;
use codescribe_core::audio::recorder::probe_input_capture_path;

#[tokio::main]
async fn main() -> Result<()> {
    ensure!(
        std::env::var("CODESCRIBE_E2E_CAPTURE_VIA_DEVICE").as_deref() == Ok("1"),
        "explicit capture opt-in required"
    );
    ensure!(
        std::env::var("AUDIO_INPUT_DEVICE").as_deref() == Ok("BlackHole 2ch"),
        "explicit BlackHole input required"
    );
    let mut args = std::env::args_os().skip(1);
    let wav = PathBuf::from(args.next().context("WAV path required")?);
    let player = PathBuf::from(args.next().context("compiled player path required")?);
    ensure!(args.next().is_none(), "unexpected arguments");
    ensure!(wav.is_file() && player.is_file(), "WAV or player missing");
    let reader = hound::WavReader::open(&wav)?;
    let seconds = reader.duration() as f64 / f64::from(reader.spec().sample_rate.max(1));
    ensure!(
        (5.0..=60.0).contains(&seconds),
        "requires 5–60 seconds of known speech"
    );
    let idle = Command::new("python3")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/bus-demux.py"))
        .arg("--assert-install-idle")
        .status()?;
    ensure!(idle.success(), "app must be idle before calibration");
    let path = probe_input_capture_path()?;
    ensure!(path.device_name == "BlackHole 2ch", "refuse another input");
    tracing_subscriber::fmt::init();
    let controller = RecordingController::new_without_keychain();
    let playback = async move {
        tokio::time::sleep(Duration::from_millis(750)).await;
        tokio::task::spawn_blocking(move || {
            Command::new(player).arg("BlackHole 2ch").arg(wav).status()
        })
        .await
        .context("player task")?
        .context("player launch")
    };
    // Await both sides, including capture Stop, even if playback fails.
    // A playback failure is never reported as a verified measurement.
    let (calibration, playback) = tokio::join!(
        controller.capture_energy_calibration(Duration::from_secs_f64(seconds + 3.0)),
        playback
    );
    let report = calibration?;
    ensure!(
        playback?.success(),
        "playback failed; stored measurement is not verified against this WAV"
    );
    ensure!(
        report.device_name == path.device_name && report.sample_rate == path.sample_rate,
        "capture path changed"
    );
    println!("{report:?}");
    Ok(())
}
