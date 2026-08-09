//! Opt-in qube donor: persist session WAV + delivered transcript for lexicon mining.
//!
//! When `CODESCRIBE_QUBE_DONOR=on`, each stop writes a date-subfolder pair under
//! `~/.codescribe/qube_inbox/<YYYY-MM-DD>/<session_ts>.{wav,txt}` so `qube-daemon`
//! (`--input ~/.codescribe/qube_inbox`) can mine lexicon candidates even when
//! `FINAL_PASS_MODE=off` (donor never runs Whisper — files only).
//!
//! Default is hard-off. Persist failures log a warning and never fail delivery.

use chrono::{DateTime, Local};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::config::Config;

const ENV_KEY: &str = "CODESCRIBE_QUBE_DONOR";
const INBOX_DIRNAME: &str = "qube_inbox";

/// Whether opt-in qube donor is enabled (`on`/`1`/`true`/`yes`). Default: off.
pub fn qube_donor_enabled() -> bool {
    match std::env::var(ENV_KEY) {
        Ok(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "on" | "1" | "true" | "yes"
        ),
        Err(_) => false,
    }
}

/// Base inbox directory: `~/.codescribe/qube_inbox` (or `CODESCRIBE_DATA_DIR/qube_inbox`).
pub fn qube_inbox_base_dir() -> PathBuf {
    Config::config_dir().join(INBOX_DIRNAME)
}

/// Result of a donor attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QubeDonorPaths {
    pub wav: PathBuf,
    pub txt: PathBuf,
}

/// Persist WAV + delivered transcript into the qube-daemon date-subfolder layout.
///
/// Returns:
/// - `Ok(Some(paths))` when both files were written
/// - `Ok(None)` when skipped (disabled, empty text, missing/empty WAV, zero samples)
/// - `Err(_)` on I/O failure (caller must not fail the delivery pipeline)
pub fn persist_qube_donor_pair(
    wav_src: &Path,
    delivered_text: &str,
    timestamp: DateTime<Local>,
) -> Result<Option<QubeDonorPaths>, String> {
    if !qube_donor_enabled() {
        return Ok(None);
    }

    let text = delivered_text.trim();
    if text.is_empty() {
        debug!("qube donor skipped: empty delivered transcript");
        return Ok(None);
    }

    if !wav_src.exists() {
        return Err(format!(
            "qube donor: source WAV missing: {}",
            wav_src.display()
        ));
    }

    let meta = fs::metadata(wav_src)
        .map_err(|e| format!("qube donor: stat {}: {e}", wav_src.display()))?;
    if meta.len() == 0 {
        debug!("qube donor skipped: zero-byte WAV");
        return Ok(None);
    }

    // Non-empty samples gate — skip silent/header-only files when parseable.
    match wav_sample_count(wav_src) {
        Ok(0) => {
            debug!("qube donor skipped: WAV has zero samples");
            return Ok(None);
        }
        Ok(_) => {}
        Err(e) => {
            // Non-WAV or unreadable header: still allow if file has payload bytes
            // (tests sometimes write minimal fixtures). Prefer warn + continue only
            // when len > 44 (min PCM header).
            if meta.len() <= 44 {
                debug!("qube donor skipped: unreadable short WAV ({e})");
                return Ok(None);
            }
            debug!("qube donor: WAV sample count unavailable ({e}); copying by size");
        }
    }

    let day = timestamp.format("%Y-%m-%d").to_string();
    let day_dir = qube_inbox_base_dir().join(&day);
    fs::create_dir_all(&day_dir)
        .map_err(|e| format!("qube donor: create {}: {e}", day_dir.display()))?;

    let session_ts = timestamp.format("%Y%m%dT%H%M%S").to_string();
    let (wav_dest, txt_dest) = unique_pair_paths(&day_dir, &session_ts);

    fs::copy(wav_src, &wav_dest)
        .map_err(|e| format!("qube donor: copy wav → {}: {e}", wav_dest.display()))?;

    match fs::File::create(&txt_dest) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(text.as_bytes()) {
                let _ = fs::remove_file(&wav_dest);
                let _ = fs::remove_file(&txt_dest);
                return Err(format!("qube donor: write txt {}: {e}", txt_dest.display()));
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&wav_dest);
            return Err(format!(
                "qube donor: create txt {}: {e}",
                txt_dest.display()
            ));
        }
    }

    info!(
        "qube donor wrote pair wav={} txt={}",
        wav_dest.display(),
        txt_dest.display()
    );
    Ok(Some(QubeDonorPaths {
        wav: wav_dest,
        txt: txt_dest,
    }))
}

/// Best-effort donor call for the stop path: never returns Err to the pipeline.
pub fn try_persist_qube_donor_at_stop(
    wav_src: Option<&Path>,
    delivered_text: &str,
    timestamp: DateTime<Local>,
) {
    if !qube_donor_enabled() {
        return;
    }
    let Some(path) = wav_src else {
        debug!("qube donor skipped: no audio path on stop");
        return;
    };
    match persist_qube_donor_pair(path, delivered_text, timestamp) {
        Ok(Some(paths)) => {
            info!(
                "qube donor pair ready for lexicon mining: {}",
                paths.txt.display()
            );
        }
        Ok(None) => {}
        Err(e) => {
            warn!("qube donor persist failed (delivery unaffected): {e}");
        }
    }
}

/// Pick a free `<session_ts>[_n].{wav,txt}` pair inside `day_dir`.
///
/// Second-resolution timestamps collide when two sessions stop within the same
/// second, so a numeric suffix is appended. Both extensions must be free
/// together: `qube-daemon` pairs files by stem, so a half-taken stem would mate
/// one session's audio with another's transcript.
fn unique_pair_paths(day_dir: &Path, session_ts: &str) -> (PathBuf, PathBuf) {
    let base_wav = day_dir.join(format!("{session_ts}.wav"));
    let base_txt = day_dir.join(format!("{session_ts}.txt"));
    if !base_wav.exists() && !base_txt.exists() {
        return (base_wav, base_txt);
    }
    for i in 1..=10_000 {
        let wav = day_dir.join(format!("{session_ts}_{i}.wav"));
        let txt = day_dir.join(format!("{session_ts}_{i}.txt"));
        if !wav.exists() && !txt.exists() {
            return (wav, txt);
        }
    }
    // Extremely unlikely collision path — fall back to base names.
    (base_wav, base_txt)
}

/// Frame count from the WAV header, used to skip header-only or silent captures.
/// `Err` means the file could not be parsed as WAV at all — the caller treats
/// that as "unknown", not as "empty".
fn wav_sample_count(path: &Path) -> Result<u64, String> {
    let reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    Ok(reader.duration() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn write_test_wav(path: &Path, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        for &s in samples {
            writer.write_sample(s).expect("sample");
        }
        writer.finalize().expect("finalize");
    }

    #[test]
    #[serial]
    fn donor_optin_disabled_by_default() {
        let _g = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var(ENV_KEY);
        }
        assert!(!qube_donor_enabled());
    }

    #[test]
    #[serial]
    fn donor_optin_parses_on_off() {
        let _g = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var(ENV_KEY, "on");
        }
        assert!(qube_donor_enabled());
        unsafe {
            std::env::set_var(ENV_KEY, "off");
        }
        assert!(!qube_donor_enabled());
        unsafe {
            std::env::remove_var(ENV_KEY);
        }
    }

    #[test]
    #[serial]
    fn donor_optin_writes_wav_txt_layout_when_on() {
        let _g = env_lock().lock().unwrap();
        let temp = tempfile::tempdir().expect("temp");
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp.path());
            std::env::set_var(ENV_KEY, "on");
        }

        let src = temp.path().join("session-src.wav");
        write_test_wav(&src, &[100, -100, 200, -200, 50]);
        let ts = Local::now();
        let delivered = "Dawkowanie amoksycyliny u psa.";

        let paths = persist_qube_donor_pair(&src, delivered, ts)
            .expect("persist ok")
            .expect("should write pair");

        let day = ts.format("%Y-%m-%d").to_string();
        assert!(
            paths
                .wav
                .to_string_lossy()
                .contains(&format!("qube_inbox/{day}/")),
            "wav path must use date subfolder: {}",
            paths.wav.display()
        );
        assert_eq!(paths.wav.extension().and_then(|e| e.to_str()), Some("wav"));
        assert_eq!(paths.txt.extension().and_then(|e| e.to_str()), Some("txt"));
        assert_eq!(
            paths.wav.file_stem(),
            paths.txt.file_stem(),
            "qube-daemon collect_pairs requires matching stems"
        );
        assert!(paths.wav.metadata().expect("meta").len() > 0);
        assert_eq!(fs::read_to_string(&paths.txt).expect("read txt"), delivered);
        assert!(wav_sample_count(&paths.wav).expect("samples") > 0);

        unsafe {
            std::env::remove_var(ENV_KEY);
            std::env::remove_var("CODESCRIBE_DATA_DIR");
        }
    }

    #[test]
    #[serial]
    fn donor_optin_off_writes_no_files() {
        let _g = env_lock().lock().unwrap();
        let temp = tempfile::tempdir().expect("temp");
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp.path());
            std::env::remove_var(ENV_KEY);
        }

        let src = temp.path().join("session-src.wav");
        write_test_wav(&src, &[1, 2, 3]);
        let result = persist_qube_donor_pair(&src, "hello", Local::now()).expect("ok");
        assert!(result.is_none());
        assert!(
            !temp.path().join("qube_inbox").exists(),
            "default-off must not create qube_inbox"
        );

        unsafe {
            std::env::remove_var("CODESCRIBE_DATA_DIR");
        }
    }

    #[test]
    #[serial]
    fn donor_optin_skips_zero_sample_wav() {
        let _g = env_lock().lock().unwrap();
        let temp = tempfile::tempdir().expect("temp");
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp.path());
            std::env::set_var(ENV_KEY, "on");
        }
        let src = temp.path().join("empty.wav");
        write_test_wav(&src, &[]);
        let result = persist_qube_donor_pair(&src, "text", Local::now()).expect("ok");
        assert!(result.is_none());
        unsafe {
            std::env::remove_var(ENV_KEY);
            std::env::remove_var("CODESCRIBE_DATA_DIR");
        }
    }

    /// Donor path is pure file I/O — never a Whisper stop-path invocation surface.
    #[test]
    #[serial]
    fn donor_optin_zero_whisper_surface_when_final_pass_off() {
        let _g = env_lock().lock().unwrap();
        // Structural contract: Off routing skips Whisper; donor only copies files.
        assert_eq!(crate::config::FinalPassRoutingMode::Off.as_str(), "off");
        // Runtime proof: `persist_qube_donor_pair` only uses fs/hound — no STT.
        let temp = tempfile::tempdir().expect("temp");
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp.path());
            std::env::set_var(ENV_KEY, "on");
            std::env::set_var("FINAL_PASS_MODE", "off");
        }
        let src = temp.path().join("s.wav");
        write_test_wav(&src, &[10, 20, 30, 40]);
        let paths = persist_qube_donor_pair(&src, "donor text", Local::now())
            .expect("ok")
            .expect("pair");
        assert!(paths.wav.exists());
        assert!(paths.txt.exists());
        // No whisper timing side effects from donor work.
        let timing = crate::stt::whisper::take_final_pass_timing();
        assert_eq!(timing, crate::stt::whisper::FinalPassTiming::default());
        unsafe {
            std::env::remove_var(ENV_KEY);
            std::env::remove_var("FINAL_PASS_MODE");
            std::env::remove_var("CODESCRIBE_DATA_DIR");
        }
    }
}
