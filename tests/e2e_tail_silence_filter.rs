//! E2E coverage for the Silero tail-silence Whisper post-filter.
//!
//! Run with:
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_tail_silence_filter -- --nocapture

use std::path::{Path, PathBuf};

use codescribe::whisper::LocalWhisperEngine;
use codescribe_core::pipeline::contracts::{FileTranscriptionOptions, TranscriptionConfidenceFlag};
use serial_test::serial;
use tempfile::TempDir;

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{
    STT_OPT_IN_ENV, discover_local_whisper_model, model_discovery_hint, normalize_transcript,
    skip_unless_opt_in,
};

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: these tests are serialized and intentionally override env.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }

    fn unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: these tests are serialized and intentionally override env.
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            // SAFETY: these tests are serialized and intentionally override env.
            unsafe { std::env::set_var(self.key, prev) };
        } else {
            // SAFETY: these tests are serialized and intentionally override env.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn canonical_wav_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/data_assets/01_no-to-dobra.wav")
}

fn load_wav(path: &Path) -> (Vec<f32>, u32) {
    let reader =
        hound::WavReader::open(path).unwrap_or_else(|e| panic!("open {}: {}", path.display(), e));
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;

    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .into_samples::<i16>()
            .map(|sample| sample.unwrap() as f32 / i16::MAX as f32)
            .collect(),
        (hound::SampleFormat::Int, 24 | 32) => reader
            .into_samples::<i32>()
            .map(|sample| sample.unwrap() as f32 / i32::MAX as f32)
            .collect(),
        (hound::SampleFormat::Float, _) => reader
            .into_samples::<f32>()
            .map(|sample| sample.unwrap())
            .collect(),
        _ => panic!(
            "Unsupported WAV format {:?} {}bit",
            spec.sample_format, spec.bits_per_sample
        ),
    };

    if spec.channels == 2 {
        let mono: Vec<f32> = samples.iter().step_by(2).copied().collect();
        (mono, sample_rate)
    } else {
        (samples, sample_rate)
    }
}

fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).unwrap_or_else(|e| panic!("create wav: {e}"));
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let pcm = (clamped * i16::MAX as f32).round() as i16;
        writer.write_sample(pcm).expect("write sample");
    }
    writer.finalize().expect("finalize wav");
}

fn make_tail_silence_fixture() -> (TempDir, PathBuf) {
    let source = canonical_wav_path();
    assert!(
        source.exists(),
        "missing canonical WAV asset: {}",
        source.display()
    );

    let (samples, sample_rate) = load_wav(&source);
    let speech_len = ((sample_rate as f32) * 5.0) as usize;
    let speech_len = speech_len.min(samples.len());
    let silence_len = ((sample_rate as f32) * 6.0) as usize;

    let mut combined = samples[..speech_len].to_vec();
    combined.extend(std::iter::repeat_n(0.0f32, silence_len));

    let tmp = tempfile::tempdir().expect("tempdir");
    let wav_path = tmp.path().join("tail_silence_fixture.wav");
    write_wav(&wav_path, &combined, sample_rate);
    (tmp, wav_path)
}

fn contains_tail_hallucination(text: &str) -> bool {
    let normalized = normalize_transcript(text).to_lowercase();
    ["dziękuję", "subscribe", "thank you", "like and subscribe"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

#[test]
#[serial]
fn e2e_tail_silence_filter_smoke() {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "tail-silence filter E2E",
        "Set CODESCRIBE_E2E_STT=1 when validating real-model tail-drop behavior.",
    ) {
        return;
    }

    let _tail_drop = EnvGuard::unset("CODESCRIBE_TAIL_DROP_ENABLED");
    let _tail_sec = EnvGuard::unset("CODESCRIBE_TAIL_SILENCE_SEC");
    let _gap_sec = EnvGuard::unset("CODESCRIBE_UTTERANCE_GAP_SEC");

    let found = match discover_local_whisper_model() {
        Some(found) => found,
        None => {
            let home = home_dir();
            panic!(
                "No complete Whisper model found.\n{}",
                model_discovery_hint(&home)
            );
        }
    };

    let (_tmp, wav_path) = make_tail_silence_fixture();
    let mut engine = LocalWhisperEngine::new(&found.path).expect("load local whisper");
    let verdict = engine
        .transcribe_file_with_language(&wav_path, Some("pl"), FileTranscriptionOptions::default())
        .expect("transcribe tail-silence fixture");

    println!("Enabled transcript: {}", verdict.text);
    println!("Enabled flags: {:?}", verdict.confidence_flags);

    assert!(
        !contains_tail_hallucination(&verdict.text),
        "tail filter should drop outro hallucinations, got: {}",
        verdict.text
    );
    assert!(verdict.confidence_flags.iter().any(|flag| matches!(
        flag,
        TranscriptionConfidenceFlag::SileroDroppedTailHallucinations { count } if *count >= 1
    )));
}

#[test]
#[serial]
fn e2e_tail_silence_toggle_smoke_when_disabled() {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "tail-silence toggle E2E",
        "Set CODESCRIBE_E2E_STT=1 when validating the opt-out toggle.",
    ) {
        return;
    }

    let _tail_drop = EnvGuard::set("CODESCRIBE_TAIL_DROP_ENABLED", "0");
    let _tail_sec = EnvGuard::unset("CODESCRIBE_TAIL_SILENCE_SEC");
    let _gap_sec = EnvGuard::unset("CODESCRIBE_UTTERANCE_GAP_SEC");

    let found = match discover_local_whisper_model() {
        Some(found) => found,
        None => {
            let home = home_dir();
            panic!(
                "No complete Whisper model found.\n{}",
                model_discovery_hint(&home)
            );
        }
    };

    let (_tmp, wav_path) = make_tail_silence_fixture();
    let mut engine = LocalWhisperEngine::new(&found.path).expect("load local whisper");
    let verdict = engine
        .transcribe_file_with_language(&wav_path, Some("pl"), FileTranscriptionOptions::default())
        .expect("transcribe tail-silence fixture");

    println!("Disabled transcript: {}", verdict.text);
    println!("Disabled flags: {:?}", verdict.confidence_flags);

    assert!(
        contains_tail_hallucination(&verdict.text),
        "disabled tail-drop path should keep the outro hallucination for regression coverage, got: {}",
        verdict.text
    );
    assert!(!verdict.confidence_flags.iter().any(|flag| matches!(
        flag,
        TranscriptionConfidenceFlag::SileroDroppedTailHallucinations { .. }
    )));
}
