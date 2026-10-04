//! CLI file verdicts retain a source reference without expanding audio into sessions.

#![cfg(target_os = "macos")]

use std::fs;
use std::path::Path;

use codescribe::presentation::cli_transcript_lane::CliTranscriptLane;
use codescribe::presentation::transcript_bus::TranscriptMode;

fn lane(id: &str, root: &Path) -> CliTranscriptLane {
    CliTranscriptLane::open_at(
        id.to_string(),
        TranscriptMode::Dictation,
        root.join(format!("{id}-bus.jsonl")),
    )
    .expect("open lane at temp bus")
}

fn pcm16_wav(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
    let data_size = u32::try_from(samples.len() * 2).expect("fixture");
    let riff_size = 36 + data_size;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&riff_size.to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn write_wav(path: &Path, samples: &[i16]) {
    fs::write(path, pcm16_wav(16_000, samples)).expect("write wav fixture");
}

#[test]
fn cli_file_verdict_references_wav_and_container_without_session_audio() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    let wav = temp.path().join("source.wav");
    let container = temp.path().join("source.mov");
    write_wav(&wav, &[0, 32, -32, 64]);
    fs::write(&container, b"container reference fixture").unwrap();
    for (id, source) in [
        ("cli-reference-wav", &wav),
        ("cli-reference-mov", &container),
    ] {
        let resolved = lane(id, temp.path())
            .retain_source_reference_at(source, &sessions)
            .unwrap();
        assert_eq!(resolved, source.canonicalize().unwrap());
        assert!(!sessions.join(format!("{id}.wav")).exists());
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(sessions.join(format!("{id}.source.json"))).unwrap())
                .unwrap();
        assert_eq!(
            receipt["source_path"].as_str(),
            source.canonicalize().unwrap().to_str()
        );
    }
}
