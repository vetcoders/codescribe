use std::path::{Path, PathBuf};

use codescribe_core::pipeline::contracts::{
    RawTranscript, TranscriptSegment, TranscriptionConfidenceFlag, TranscriptionEngineMode,
    TranscriptionEngineVerdict, TranscriptionSource, TranscriptionVerdict, VadVerdict,
};
use codescribe_core::vad::{VadConfig, classify_windows, extract_speech};
use codescribe_core::whisper::map_whisper_segments_to_silero;

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

fn speech_plus_tail_silence_samples() -> (Vec<f32>, u32, f32) {
    let source = canonical_wav_path();
    assert!(
        source.exists(),
        "missing canonical WAV asset: {}",
        source.display()
    );

    let (samples, sample_rate) = load_wav(&source);
    let speech_len = ((sample_rate as f32) * 5.0) as usize;
    let speech_len = speech_len.min(samples.len());
    let speech_sec = speech_len as f32 / sample_rate as f32;
    let silence_len = ((sample_rate as f32) * 6.0) as usize;

    let mut combined = samples[..speech_len].to_vec();
    combined.extend(std::iter::repeat_n(0.0f32, silence_len));
    (combined, sample_rate, speech_sec)
}

#[test]
fn tail_silence_contract_real_vad_timeline_drops_synthetic_tail_hallucination() {
    let (samples, sample_rate, speech_sec) = speech_plus_tail_silence_samples();
    let (_speech_only, stats) = extract_speech(&samples, sample_rate);
    let vad_config = VadConfig::default();
    let timeline = classify_windows(&stats.probabilities, &vad_config);

    assert!(
        timeline.overlaps_trailing_silence(speech_sec + 2.0, speech_sec + 2.5),
        "real VAD timeline should mark the appended tail as trailing silence"
    );

    let original = vec![
        TranscriptSegment {
            text: "To jest wypowiedź".to_string(),
            start_ts: 0.5,
            end_ts: 1.0,
        },
        TranscriptSegment {
            text: "Dziękuję za uwagę".to_string(),
            start_ts: speech_sec + 2.0,
            end_ts: speech_sec + 2.4,
        },
    ];
    let outcome = map_whisper_segments_to_silero(&original, &timeline, &vad_config);

    assert_eq!(outcome.dropped_count, 1);
    assert_eq!(outcome.text, "To jest wypowiedź");
    assert_eq!(outcome.segments.len(), 1);
    assert!(!outcome.text.to_lowercase().contains("dziękuję"));

    let vad_verdict = VadVerdict {
        speech_pct: stats.speech_pct,
        speech_windows: stats.speech_windows,
        total_windows: stats.total_windows,
        no_speech: false,
        no_speech_reason: stats.no_speech_reason.clone(),
        sparkline: stats.sparkline.clone(),
    };
    let raw = RawTranscript {
        text: outcome.text.clone(),
        segments: outcome.segments.clone(),
        ..Default::default()
    };
    let verdict = TranscriptionVerdict::from_parts_with_silero_drops(
        outcome.text,
        raw,
        Some(vad_verdict),
        TranscriptionSource::LocalFinalPass,
        TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
        None,
        outcome.dropped_count,
    );

    assert!(verdict.confidence_flags.iter().any(|flag| matches!(
        flag,
        TranscriptionConfidenceFlag::SileroDroppedTailHallucinations { count } if *count == 1
    )));
}
