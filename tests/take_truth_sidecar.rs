//! Take truth sidecar — the April `.truth.json` end-to-end through the core
//! contract (`core/pipeline/take_truth.rs`).
//!
//! The fixture is a byte-verbatim copy of the Founder's reference sidecar
//! (`~/.codescribe/transcriptions/2026-04-21/211316_ogolnie-plan-ktory_raw.txt.truth.json`,
//! "no o to to to — o takie wlasnie mi chodzilo"). It must deserialize through
//! `read_truth_sidecar`, keep the twelve April values, round-trip through
//! `write_truth_sidecar`, and never carry transcript text.

use std::fs;
use std::path::PathBuf;

use codescribe_core::pipeline::take_truth::{read_truth_sidecar, write_truth_sidecar};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/truth_sidecar_20260421.json")
}

/// The verbatim April file parses through `read_truth_sidecar`, keeps the
/// twelve April fields, round-trips on disk, and contains no `"text"` key.
#[test]
fn april_sidecar_roundtrips_through_core_contract() {
    let fixture = fixture_path();

    // The fixture must contain no transcript text — truth, not content.
    let raw = fs::read_to_string(&fixture).expect("fixture reads");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("fixture is JSON");
    let object = value.as_object().expect("fixture is a JSON object");
    assert!(
        !object.contains_key("text"),
        "fixture must not contain a \"text\" key"
    );
    assert_eq!(object.len(), 12, "April sidecar has exactly twelve keys");

    // Stage the fixture under a real sidecar name so `read_truth_sidecar`
    // resolves it from the transcript path (`<file name>.truth.json`).
    let dir = tempfile::tempdir().expect("tempdir");
    let transcript = dir.path().join("211316_ogolnie-plan-ktory_raw.txt");
    fs::write(&transcript, "placeholder transcript bytes").expect("write transcript");
    fs::copy(
        &fixture,
        dir.path()
            .join("211316_ogolnie-plan-ktory_raw.txt.truth.json"),
    )
    .expect("stage fixture");

    let truth = read_truth_sidecar(&transcript).expect("fixture parses via read_truth_sidecar");

    // The twelve April fields, verbatim.
    assert_eq!(truth.source, "local_final_pass");
    assert_eq!(truth.engine, "local_whisper");
    assert_eq!(truth.mode, "raw");
    assert_eq!(truth.fallback_class, None);
    assert!(!truth.fallback_used);
    assert!((truth.vad_speech_pct - 61.714287_f32).abs() < 1e-6);
    assert_eq!(truth.no_speech_reason, None);
    assert!((truth.avg_logprob.expect("avg_logprob") - (-0.26560482_f32)).abs() < 1e-6);
    assert!(truth.confidence_flags.is_empty());
    assert_eq!(truth.sparkline.chars().count(), 350);
    assert_eq!(truth.commit_trigger, None);
    assert_eq!(
        truth.display_status.as_deref(),
        Some("Final-pass local • Transcript")
    );
    // April did not know it would become history: absent key reads as v1.
    assert_eq!(truth.schema_version, 1);

    // Write beside a fresh artifact and read back — full fidelity.
    let archive_txt = dir.path().join("copy_raw.txt");
    fs::write(&archive_txt, "placeholder transcript bytes").expect("write archive txt");
    let written = write_truth_sidecar(&archive_txt, &truth).expect("write sidecar");
    assert!(
        written.ends_with("copy_raw.txt.truth.json"),
        "sidecar lands beside the artifact as <file name>.truth.json"
    );
    let restored = read_truth_sidecar(&archive_txt).expect("read back");
    assert_eq!(restored, truth);

    // The re-serialized v1 file still has no text and no schema bump.
    let rewritten_raw = fs::read_to_string(&written).expect("rewritten sidecar reads");
    let rewritten: serde_json::Value = serde_json::from_str(&rewritten_raw).unwrap();
    let rewritten = rewritten.as_object().unwrap();
    assert!(!rewritten.contains_key("text"));
    assert!(!rewritten.contains_key("schema_version"));
    assert_eq!(rewritten.len(), 12, "v1 shape survives the disk round-trip");
}
