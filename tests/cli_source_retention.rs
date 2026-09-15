//! CLI file retention: one inode for identical WAV bytes, always a real RIFF.
//!
//! Authored for W1; W2 runs `cargo test --test cli_source_retention`.
//! App-side `publish_retained_audio` writes a fresh inode and `renameat`s —
//! nothing in-tree opens `sessions/*.wav` for in-place writing, which is why
//! hardlinks are safe. These cases pin that from the CLI side.

#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

fn index_files(sessions: &Path) -> Vec<PathBuf> {
    let index = sessions.join(".index");
    fs::read_dir(&index)
        .expect("index dir")
        .map(|entry| entry.expect("index entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.starts_with('.'))
        })
        .collect()
}

#[test]
fn same_riff_source_two_session_names_share_one_inode() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    let source = temp.path().join("source.wav");
    write_wav(&source, &[0, 32, -32, 64]);

    let first = lane("retain-a1", temp.path())
        .retain_source_wav_at(&source, &sessions)
        .expect("retain first");
    let second = lane("retain-a2", temp.path())
        .retain_source_wav_at(&source, &sessions)
        .expect("retain second");

    assert_eq!(first, sessions.join("retain-a1.wav"));
    assert_eq!(second, sessions.join("retain-a2.wav"));
    assert_ne!(first, second);

    let first_meta = fs::metadata(&first).expect("first meta");
    let second_meta = fs::metadata(&second).expect("second meta");
    assert_eq!(
        first_meta.ino(),
        second_meta.ino(),
        "identical RIFF bytes must share one inode"
    );
    assert_eq!(first_meta.nlink(), 2);
    assert_eq!(second_meta.nlink(), 2);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&source).unwrap());
    assert_eq!(fs::read(&second).unwrap(), fs::read(&first).unwrap());
}

#[test]
fn non_riff_source_is_retained_as_riff() {
    if Command::new("afconvert")
        .arg("-h")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        panic!(
            "afconvert missing; ignore this test with --skip non_riff_source_is_retained_as_riff"
        );
    }

    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    let wav = temp.path().join("tone.wav");
    let m4a = temp.path().join("tone.m4a");
    write_wav(&wav, &[0, 1000, -1000, 2000, -2000, 0]);
    let status = Command::new("afconvert")
        .args(["-f", "m4af", "-d", "aac"])
        .arg(&wav)
        .arg(&m4a)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn afconvert");
    assert!(status.success(), "afconvert failed: {status}");
    let header = fs::read(&m4a).expect("read m4a");
    assert!(
        !(header.starts_with(b"RIFF") && header.get(8..12) == Some(&b"WAVE"[..])),
        "fixture must be a non-RIFF container"
    );

    let dest = lane("retain-b1", temp.path())
        .retain_source_wav_at(&m4a, &sessions)
        .expect("retain m4a");
    let retained = fs::read(&dest).expect("read retained");
    assert!(
        retained.starts_with(b"RIFF") && retained.get(8..12) == Some(&b"WAVE"[..]),
        "non-RIFF source must land as a real WAV"
    );
    hound::WavReader::open(&dest).expect("hound opens retained WAV");
}

#[test]
fn stale_index_missing_canonical_rewrites_and_copies() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    let source = temp.path().join("source.wav");
    write_wav(&source, &[7, 8, 9, 10]);

    let first = lane("retain-c1", temp.path())
        .retain_source_wav_at(&source, &sessions)
        .expect("retain first");
    let indexes = index_files(&sessions);
    assert_eq!(indexes.len(), 1);
    assert_eq!(
        fs::read_to_string(&indexes[0]).unwrap().trim(),
        "retain-c1.wav"
    );
    fs::remove_file(&first).expect("remove canonical");

    let second = lane("retain-c2", temp.path())
        .retain_source_wav_at(&source, &sessions)
        .expect("retain after canonical gone");
    assert!(second.is_file());
    assert_eq!(fs::read(&second).unwrap(), fs::read(&source).unwrap());
    assert_eq!(
        fs::read_to_string(&indexes[0]).unwrap().trim(),
        "retain-c2.wav"
    );
}

#[test]
fn last_session_wav_identity_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    let source = temp.path().join("source.wav");
    write_wav(&source, &[1, 2, 3, 4]);

    let err = lane("last_session", temp.path())
        .retain_source_wav_at(&source, &sessions)
        .expect_err("last_session dest must be refused");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(!sessions.join("last_session.wav").exists());
}

#[test]
fn existing_dest_is_replaced_without_inplace_truncation() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    let dest = sessions.join("retain-e1.wav");
    let old_bytes = pcm16_wav(16_000, &[11, 12, 13]);
    let new_bytes = pcm16_wav(16_000, &[21, 22, 23, 24]);
    fs::write(&dest, &old_bytes).unwrap();
    let hold = sessions.join("retain-hold.wav");
    fs::hard_link(&dest, &hold).expect("hold old inode");
    let old_ino = fs::metadata(&dest).unwrap().ino();
    let source = temp.path().join("source.wav");
    fs::write(&source, &new_bytes).unwrap();

    let retained = lane("retain-e1", temp.path())
        .retain_source_wav_at(&source, &sessions)
        .expect("replace dest");
    assert_eq!(retained, dest);
    assert_eq!(fs::read(&dest).unwrap(), new_bytes);
    assert_eq!(fs::metadata(&hold).unwrap().ino(), old_ino);
    assert_eq!(fs::read(&hold).unwrap(), old_bytes);
    assert_ne!(fs::metadata(&dest).unwrap().ino(), old_ino);
}
