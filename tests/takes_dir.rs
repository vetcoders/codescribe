//! Scratch take WAVs land under `$CODESCRIBE_DATA_DIR/takes`, not OS temp.
//!
//! Authored in W1; W2 runs `cargo test --test takes_dir`. No microphone:
//! `Recorder::{start,stop,snapshot_wav}` need a live capture device, so this
//! binary calls `takes_dir` and the microphone-free `SpillSink` wrapper
//! `spill_take_wav_for_tests`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: this integration-test binary owns CODESCRIBE_DATA_DIR.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            // SAFETY: restore the value this test replaced.
            unsafe { std::env::set_var(self.key, prev) };
        } else {
            // SAFETY: restore absence when the test introduced the var.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

fn recording_names_in(dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return names;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with("codescribe_recording_") {
            names.insert(name.to_string());
        }
    }
    names
}

#[test]
fn spill_wav_lands_under_codescribe_takes_not_temp_dir() {
    let data = tempfile::tempdir().expect("CODESCRIBE_DATA_DIR tempfile");
    let _guard = EnvGuard::set(
        "CODESCRIBE_DATA_DIR",
        data.path().to_str().expect("tempfile path is UTF-8"),
    );

    let takes = codescribe_core::audio::recorder::takes_dir().expect("takes_dir");
    let expected = codescribe_core::config::Config::config_dir().join("takes");
    assert_eq!(takes, expected, "takes_dir must honour CODESCRIBE_DATA_DIR");
    assert!(
        takes.is_dir(),
        "takes_dir must create the directory: {}",
        takes.display()
    );
    assert!(
        takes.ends_with("takes"),
        "takes helper must append the takes segment: {}",
        takes.display()
    );

    let temp = std::env::temp_dir();
    let before = recording_names_in(&temp);

    let samples: Vec<i16> = vec![i16::MIN, -1, 0, 1, i16::MAX, 1234, -4321];
    let path: PathBuf =
        codescribe_core::audio::recorder::spill_take_wav_for_tests(&samples, 16_000)
            .expect("spill take wav");

    assert!(
        path.starts_with(&takes),
        "spill wav must land under takes/: got {}",
        path.display()
    );
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .expect("utf-8 wav name");
    assert!(
        file_name.starts_with("codescribe_recording_") && file_name.ends_with(".wav"),
        "filename must stay codescribe_recording_<ms>.wav, got {file_name}"
    );
    assert!(
        path.is_file(),
        "spill must produce a file: {}",
        path.display()
    );

    let read: Vec<i16> = hound::WavReader::open(&path)
        .expect("open spilled wav")
        .into_samples::<i16>()
        .map(|s| s.expect("sample"))
        .collect();
    assert_eq!(read, samples, "spill must round-trip PCM");

    let after = recording_names_in(&temp);
    assert_eq!(
        after, before,
        "no new codescribe_recording_* as a direct child of std::env::temp_dir()"
    );
}
