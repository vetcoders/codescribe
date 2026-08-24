//! Frozen P0-B five-Iwo conservation oracle.
//!
//! This module is compiled as a child of `presentation::transcript_bus`, so it
//! can drive the real reducer and Bus without widening the production API.  It
//! contains assertions and fixture interpretation only; occurrence admission,
//! reduction, Bus serialization and mutation decisions stay production-owned.

use super::*;
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, MutationReceipt, ObservationIdentity, ObservationProducer,
    OccurrenceIdentity as LedgerOccurrenceIdentity, RefuseReason,
};
use codescribe_core::pipeline::contracts::{
    AcousticSpanGrain, AcousticTranscriptIdentity, AcousticTranscriptSpan, EngineEvent,
};
use codescribe_core::stt::tail_provider::TailSampleRange;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST_REL: &str = "tests/fixtures/p0_b_five_iwo_manifest.json";
const EXPECTED_SCHEMA: &str = "codescribe.p0-b.five-iwo-fixture.v1";
const SESSION_ID: &str = "p0-b-five-iwo";
const CAPTURE_EPOCH: u64 = 1;

#[derive(Debug, Deserialize)]
struct FixtureManifest {
    schema: String,
    fixture: String,
    wav_sha256: String,
    sample_rate: u32,
    channels: u16,
    sample_format: String,
    sample_count: u64,
    label: String,
    expected_occurrences: usize,
    minimum_energy_integral: f64,
    minimum_valley_samples: u64,
    bursts: Vec<FixtureBurst>,
    vad_valleys: Vec<FixtureValley>,
    provenance: FixtureProvenance,
    controls: Vec<FixtureControl>,
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureBurst {
    ordinal: usize,
    label: String,
    sample_start: u64,
    sample_end: u64,
    duration_ms: f64,
    energy_integral: f64,
    mean_rms_dbfs: f64,
    peak_dbfs: f64,
    vad_open_sample: u64,
    vad_close_sample: u64,
    evidence_calibration_version: String,
}

#[derive(Debug, Deserialize)]
struct FixtureValley {
    sample_start: u64,
    sample_end: u64,
}

#[derive(Debug, Deserialize)]
struct FixtureProvenance {
    kind: String,
    generator: String,
    generator_version: u64,
    operator_recording: bool,
    derived_from_human_audio: bool,
    description: String,
}

#[derive(Debug, Deserialize)]
struct FixtureControl {
    id: String,
    mutation: String,
    expected: String,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_manifest() -> FixtureManifest {
    let path = repo_root().join(MANIFEST_REL);
    let bytes = fs::read(&path)
        .unwrap_or_else(|error| panic!("five-Iwo manifest missing at {}: {error}", path.display()));
    serde_json::from_slice(&bytes).expect("five-Iwo manifest must be valid JSON")
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path)
        .unwrap_or_else(|error| panic!("cannot read five-Iwo fixture {}: {error}", path.display()));
    format!("{:x}", Sha256::digest(bytes))
}

fn load_pcm(manifest: &FixtureManifest) -> Vec<f32> {
    let path = repo_root().join(&manifest.fixture);
    assert_eq!(
        sha256_file(&path),
        manifest.wav_sha256,
        "fixture digest drifted; regenerate with scripts/generate-five-iwo-fixture.py"
    );
    let mut reader = hound::WavReader::open(&path)
        .unwrap_or_else(|error| panic!("invalid five-Iwo WAV {}: {error}", path.display()));
    let spec = reader.spec();
    assert_eq!(spec.channels, manifest.channels);
    assert_eq!(spec.sample_rate, manifest.sample_rate);
    assert_eq!(spec.bits_per_sample, 16);
    let samples = reader
        .samples::<i16>()
        .map(|sample| sample.expect("valid PCM sample") as f32 / 32_768.0)
        .collect::<Vec<_>>();
    assert_eq!(samples.len() as u64, manifest.sample_count);
    samples
}

fn measured_energy(samples: &[f32], start: u64, end: u64) -> (f64, f64, f64) {
    let slice = &samples[start as usize..end as usize];
    let energy = slice
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>();
    let rms = (energy / slice.len() as f64).sqrt();
    let peak = slice
        .iter()
        .map(|sample| f64::from(sample.abs()))
        .fold(0.0f64, f64::max);
    let to_dbfs = |value: f64| 20.0 * value.max(1.0e-12).log10();
    (energy, to_dbfs(rms), to_dbfs(peak))
}

fn fixture_variant(mut samples: Vec<f32>, manifest: &FixtureManifest) -> Vec<f32> {
    if std::env::var("P0_B_FIXTURE_VARIANT").as_deref() == Ok("four-burst") {
        let fifth = manifest.bursts.last().expect("fifth burst in manifest");
        samples[fifth.sample_start as usize..fifth.sample_end as usize].fill(0.0);
    }
    samples
}

fn energy_qualified<'a>(samples: &[f32], manifest: &'a FixtureManifest) -> Vec<&'a FixtureBurst> {
    manifest
        .bursts
        .iter()
        .filter(|burst| {
            let (energy, _, _) = measured_energy(samples, burst.sample_start, burst.sample_end);
            burst.sample_end > burst.sample_start
                && energy >= manifest.minimum_energy_integral
                && burst.vad_open_sample == burst.sample_start
                && burst.vad_close_sample == burst.sample_end
        })
        .collect()
}

fn fixture_energy_lookup(start: u64, end: u64) -> Option<f32> {
    load_manifest()
        .bursts
        .iter()
        .find(|burst| burst.sample_start == start && burst.sample_end == end)
        .map(|burst| burst.mean_rms_dbfs as f32)
}

fn acoustic_identity(
    manifest: &FixtureManifest,
    bursts: &[&FixtureBurst],
) -> AcousticTranscriptIdentity {
    AcousticTranscriptIdentity {
        range: TailSampleRange {
            session: SESSION_ID.to_string(),
            capture_epoch: CAPTURE_EPOCH,
            sample_start: bursts.first().expect("qualified burst").sample_start,
            sample_end: bursts.last().expect("qualified burst").sample_end,
        },
        spans: bursts
            .iter()
            .map(|burst| AcousticTranscriptSpan {
                text: manifest.label.clone(),
                range: TailSampleRange {
                    session: SESSION_ID.to_string(),
                    capture_epoch: CAPTURE_EPOCH,
                    sample_start: burst.sample_start,
                    sample_end: burst.sample_end,
                },
                grain: AcousticSpanGrain::Word,
            })
            .collect(),
    }
}

fn admit_fixture_to_ledger(manifest: &FixtureManifest, bursts: &[&FixtureBurst]) -> AcousticLedger {
    let mut ledger = AcousticLedger::new();
    for burst in bursts {
        let occurrence = LedgerOccurrenceIdentity::new(
            SESSION_ID,
            CAPTURE_EPOCH,
            burst.sample_start,
            burst.sample_end,
        );
        let apple = ObservationIdentity::new(
            ObservationProducer::Apple,
            burst.ordinal as u64,
            0,
            occurrence.clone(),
        );
        assert!(
            ledger.admit(&apple, &manifest.label).is_insert(),
            "Apple must insert physical occurrence {}",
            burst.ordinal
        );
        let whisper = ObservationIdentity::new(
            ObservationProducer::Whisper,
            100 + burst.ordinal as u64,
            0,
            occurrence.clone(),
        );
        assert!(
            matches!(
                ledger.admit(&whisper, &manifest.label),
                MutationReceipt::Preserve { occurrence: held, .. } if held == occurrence
            ),
            "Whisper must attach to occurrence {}, not mint another one",
            burst.ordinal
        );
    }
    ledger
}

fn publish_through_reducer_and_bus(
    manifest: &FixtureManifest,
    bursts: &[&FixtureBurst],
) -> (String, Value) {
    use super::super::emitter::reduce_transcript_events;

    let identity = acoustic_identity(manifest, bursts);
    let text = std::iter::repeat_n(manifest.label.as_str(), bursts.len())
        .collect::<Vec<_>>()
        .join(" ");
    let event = EngineEvent::UtteranceFinal {
        utterance_id: 1,
        text: text.clone(),
        raw_text: text,
        start_ts: identity.range.sample_start as f32 / manifest.sample_rate as f32,
        end_ts: identity.range.sample_end as f32 / manifest.sample_rate as f32,
        segments: Vec::new(),
        vad_speech_pct: None,
        avg_logprob: None,
        compression_ratio: None,
        quality_gate_dropped: false,
        confidence_flags: Vec::new(),
        acoustic: Some(identity.clone()),
    };
    let reducer = reduce_transcript_events(&[event]);
    let rendered = reducer.rendered_text();

    let temp = tempfile::tempdir().expect("temporary Bus directory");
    let path = temp.path().join("events.jsonl");
    let bus = TranscriptBus::open_at_with_energy(
        TranscriptSession {
            session_id: SESSION_ID.to_string(),
            mode: TranscriptMode::Dictation,
        },
        path.clone(),
        Some(manifest.sample_rate),
        fixture_energy_lookup,
    )
    .expect("open synthetic Transcript Bus");
    bus.publish_started();
    bus.publish_draft(
        TranscriptDraftStatus::Created,
        TranscriptDraft {
            utterance_id: 1,
            text: rendered.clone(),
            start_seconds: identity.range.sample_start as f32 / manifest.sample_rate as f32,
            end_seconds: identity.range.sample_end as f32 / manifest.sample_rate as f32,
            segments: Vec::new(),
            acoustic: Some(identity),
        },
    );
    bus.publish_sealed(rendered.clone(), None);

    let raw = fs::read_to_string(path).expect("read synthetic Bus trace");
    let seal = raw
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("valid Bus JSONL"))
        .find(|event| event.get("status").and_then(Value::as_str) == Some("transcript_sealed"))
        .expect("terminal Transcript Bus seal");
    (rendered, seal)
}

fn complete_oracle_trace(manifest: &FixtureManifest) -> Value {
    let words = manifest
        .bursts
        .iter()
        .map(|burst| {
            json!({
                "text": burst.label,
                "acoustic_serial_version": 1,
                "acoustic_serial": format!("p0b-{}", burst.ordinal),
                "sample_start": burst.sample_start,
                "sample_end": burst.sample_end,
                "duration_ms": burst.duration_ms,
                "energy_integral": burst.energy_integral,
                "mean_rms_dbfs": burst.mean_rms_dbfs,
                "peak_dbfs": burst.peak_dbfs,
                "evidence_calibration_version": burst.evidence_calibration_version,
                "vad_open_sample": burst.vad_open_sample,
                "vad_close_sample": burst.vad_close_sample,
                "observation_frontier": "closed",
                "layer_decisions": [
                    {"layer": "apple", "decision": "insert", "receipt": format!("apple-{}", burst.ordinal)},
                    {"layer": "whisper", "decision": "preserve", "receipt": format!("whisper-{}", burst.ordinal)},
                    {"layer": "retained_text", "decision": "retain", "receipt": format!("retained-{}", burst.ordinal)}
                ],
                "seal_receipt": {"state": "sealed", "receipt": format!("seal-{}", burst.ordinal)},
                "post_seal_mutations": []
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": "transcript_sealed",
        "text": "Iwo Iwo Iwo Iwo Iwo",
        "energy_lookup_available": false,
        "terminal_ledger_seal": {"state": "sealed", "receipt": "terminal-p0b"},
        "words": words
    })
}

fn validate_oracle_trace(trace: &Value, expected: usize) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    if trace.get("status").and_then(Value::as_str) != Some("transcript_sealed") {
        failures.push("missing terminal transcript seal".to_string());
    }
    if trace
        .pointer("/terminal_ledger_seal/state")
        .and_then(Value::as_str)
        != Some("sealed")
    {
        failures.push("missing terminal ledger seal receipt".to_string());
    }
    let words = trace
        .get("words")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if words.len() != expected {
        failures.push(format!(
            "five-Iwo conservation: expected {expected} committed words, observed {} (four-vs-five)",
            words.len()
        ));
    }
    for (index, word) in words.iter().enumerate() {
        let ordinal = index + 1;
        for field in [
            "acoustic_serial_version",
            "acoustic_serial",
            "duration_ms",
            "energy_integral",
            "mean_rms_dbfs",
            "peak_dbfs",
            "evidence_calibration_version",
            "vad_open_sample",
            "vad_close_sample",
            "seal_receipt",
        ] {
            if word.get(field).is_none() || word.get(field).is_some_and(Value::is_null) {
                failures.push(format!(
                    "word {ordinal} missing required evidence field {field}"
                ));
            }
        }
        if word
            .get("energy_integral")
            .and_then(Value::as_f64)
            .is_none_or(|energy| energy <= 0.0)
        {
            failures.push(format!("word {ordinal} lacks positive calibrated energy"));
        }
        if word.get("observation_frontier").and_then(Value::as_str) != Some("closed") {
            failures.push(format!("word {ordinal} observation frontier is not closed"));
        }
        let layers = word
            .get("layer_decisions")
            .and_then(Value::as_array)
            .map(|history| {
                history
                    .iter()
                    .filter_map(|entry| entry.get("layer").and_then(Value::as_str))
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        for required in ["apple", "whisper", "retained_text"] {
            if !layers.contains(required) {
                failures.push(format!("word {ordinal} missing {required} layer decision"));
            }
        }
        if let Some(mutations) = word.get("post_seal_mutations").and_then(Value::as_array) {
            for mutation in mutations {
                let accepted = mutation.get("accepted").and_then(Value::as_bool) == Some(true);
                let producer = mutation
                    .get("producer")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if accepted && producer != "manual_human" {
                    failures.push(format!(
                        "word {ordinal} accepted automatic post-seal mutation from {producer}"
                    ));
                }
                if accepted
                    && producer == "manual_human"
                    && mutation.get("manual_edit_receipt").is_none()
                {
                    failures.push(format!(
                        "word {ordinal} manual edit lacks provenance receipt"
                    ));
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

#[test]
fn p0_b_fixture_provenance_digest_and_energy_are_public_synthetic() {
    let manifest = load_manifest();
    assert_eq!(manifest.schema, EXPECTED_SCHEMA);
    assert_eq!(manifest.expected_occurrences, 5);
    assert_eq!(manifest.label, "Iwo");
    assert_eq!(
        manifest.provenance.kind,
        "deterministic_mathematical_synthesis"
    );
    assert_eq!(
        manifest.provenance.generator,
        "scripts/generate-five-iwo-fixture.py"
    );
    assert_eq!(manifest.provenance.generator_version, 1);
    assert!(!manifest.provenance.operator_recording);
    assert!(!manifest.provenance.derived_from_human_audio);
    assert!(
        manifest
            .provenance
            .description
            .contains("Harmonic tone bursts")
    );
    assert!(!manifest.fixture.contains(".codescribe"));
    assert!(!manifest.fixture.contains("/Users/"));
    assert_eq!(manifest.sample_format, "pcm_s16le");

    let samples = load_pcm(&manifest);
    assert_eq!(manifest.bursts.len(), 5);
    assert_eq!(manifest.vad_valleys.len(), 5);
    for burst in &manifest.bursts {
        let (energy, rms_dbfs, peak_dbfs) =
            measured_energy(&samples, burst.sample_start, burst.sample_end);
        assert!((energy - burst.energy_integral).abs() < 1.0e-5);
        assert!((rms_dbfs - burst.mean_rms_dbfs).abs() < 1.0e-4);
        assert!((peak_dbfs - burst.peak_dbfs).abs() < 1.0e-4);
        assert!(energy >= manifest.minimum_energy_integral);
    }
    for valley in &manifest.vad_valleys {
        assert!(valley.sample_end - valley.sample_start >= manifest.minimum_valley_samples);
        assert!(
            samples[valley.sample_start as usize..valley.sample_end as usize]
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }
    let control_ids = manifest
        .controls
        .iter()
        .map(|control| control.id.as_str())
        .collect::<BTreeSet<_>>();
    for required in [
        "N1", "N2", "N3", "N4", "N5", "N6", "N7", "N8", "N9", "N10", "N11", "N12", "A1", "A2",
        "A3", "A4", "A5",
    ] {
        assert!(control_ids.contains(required), "missing control {required}");
    }
    assert!(manifest.controls.iter().all(|control| {
        !control.mutation.trim().is_empty() && !control.expected.trim().is_empty()
    }));
}

#[test]
fn p0_b_replay_no_energy_and_conflicting_label_controls_use_production_paths() {
    let manifest = load_manifest();
    let samples = load_pcm(&manifest);
    let bursts = energy_qualified(&samples, &manifest);
    let mut ledger = admit_fixture_to_ledger(&manifest, &bursts);
    let first = bursts[0];
    let occurrence = LedgerOccurrenceIdentity::new(
        SESSION_ID,
        CAPTURE_EPOCH,
        first.sample_start,
        first.sample_end,
    );
    let replay = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence.clone());
    assert!(matches!(
        ledger.admit(&replay, "Iwo"),
        MutationReceipt::Refuse {
            reason: RefuseReason::BatchDuplicate,
            ..
        }
    ));
    assert_eq!(ledger.len(), 5, "replay may not mint a sixth occurrence");

    let correction =
        ObservationIdentity::new(ObservationProducer::Whisper, 999, 1, occurrence.clone());
    let receipt = ledger.admit(&correction, "Ivo");
    assert!(matches!(receipt, MutationReceipt::Correct { .. }));
    assert_eq!(
        ledger.len(),
        5,
        "conflicting labels share one physical occurrence"
    );

    let identity = acoustic_identity(&manifest, &bursts);
    let draft = TranscriptDraft {
        utterance_id: 1,
        text: "Iwo Iwo Iwo Iwo Iwo".to_string(),
        start_seconds: 0.0,
        end_seconds: manifest.sample_count as f32 / manifest.sample_rate as f32,
        segments: Vec::new(),
        acoustic: Some(identity),
    };
    let (words, coverage) = word_spans_from_draft(&draft, |_start, _end| None);
    assert!(words.is_empty());
    assert!(!coverage.passed);
    assert_eq!(coverage.code, "lexical_span_without_voiced_energy");
}

#[test]
fn p0_b_oracle_fails_closed_for_all_required_negative_controls() {
    let manifest = load_manifest();
    let complete = complete_oracle_trace(&manifest);
    assert!(
        validate_oracle_trace(&complete, 5).is_ok(),
        "complete oracle example must prove the verifier can pass"
    );
    assert_eq!(
        complete.get("energy_lookup_available"),
        Some(&Value::Bool(false))
    );

    let mut four = complete.clone();
    four["words"].as_array_mut().unwrap().pop();
    let failures = validate_oracle_trace(&four, 5).unwrap_err().join("; ");
    assert!(failures.contains("four-vs-five"));

    let mut no_energy = complete.clone();
    no_energy["words"][0]["energy_integral"] = json!(0.0);
    assert!(
        validate_oracle_trace(&no_energy, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("positive calibrated energy"))
    );

    let mut no_vad_close = complete.clone();
    no_vad_close["words"][0]["vad_close_sample"] = Value::Null;
    assert!(
        validate_oracle_trace(&no_vad_close, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("vad_close_sample"))
    );

    let mut open_frontier = complete.clone();
    open_frontier["words"][0]["observation_frontier"] = json!("open");
    assert!(
        validate_oracle_trace(&open_frontier, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("frontier is not closed"))
    );

    let mut missing_serial = complete.clone();
    missing_serial["words"][0]
        .as_object_mut()
        .unwrap()
        .remove("acoustic_serial");
    assert!(
        validate_oracle_trace(&missing_serial, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("acoustic_serial"))
    );

    let mut missing_layer = complete.clone();
    missing_layer["words"][0]["layer_decisions"]
        .as_array_mut()
        .unwrap()
        .retain(|decision| decision["layer"] != "whisper");
    assert!(
        validate_oracle_trace(&missing_layer, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("whisper layer decision"))
    );

    let mut automatic_mutation = complete.clone();
    automatic_mutation["words"][0]["post_seal_mutations"] =
        json!([{"producer": "formatter", "accepted": true}]);
    assert!(
        validate_oracle_trace(&automatic_mutation, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("automatic post-seal mutation"))
    );

    let mut manual_mutation = complete;
    manual_mutation["words"][0]["post_seal_mutations"] = json!([{
        "producer": "manual_human",
        "accepted": true,
        "manual_edit_receipt": "manual-p0b-1"
    }]);
    assert!(validate_oracle_trace(&manual_mutation, 5).is_ok());
}

#[test]
fn p0_b_five_iwo_energy_qualified_ledger_to_delivery_conservation() {
    let manifest = load_manifest();
    let samples = fixture_variant(load_pcm(&manifest), &manifest);
    let bursts = energy_qualified(&samples, &manifest);
    assert_eq!(
        bursts.len(),
        manifest.expected_occurrences,
        "five-Iwo conservation: expected 5 energy-qualified bursts, observed {} (four-vs-five)",
        bursts.len()
    );

    let ledger = admit_fixture_to_ledger(&manifest, &bursts);
    assert_eq!(
        ledger.len(),
        5,
        "ledger must hold all five physical occurrences"
    );
    let (rendered, seal) = publish_through_reducer_and_bus(&manifest, &bursts);
    assert_eq!(
        rendered
            .split_whitespace()
            .filter(|word| word.eq_ignore_ascii_case("iwo"))
            .count(),
        5,
        "reducer/delivery must conserve all five Iwo labels"
    );

    if let Err(failures) = validate_oracle_trace(&seal, 5) {
        panic!(
            "P0-B RED — ledger-to-delivery evidence predicate is missing:\n{}",
            failures.join("\n")
        );
    }
}

#[test]
fn p0_b_terminal_seal_fences_automatic_mutation_but_allows_manual_provenance() {
    let manifest = load_manifest();
    let samples = load_pcm(&manifest);
    let bursts = energy_qualified(&samples, &manifest);
    let mut ledger = admit_fixture_to_ledger(&manifest, &bursts);
    let (_rendered, seal) = publish_through_reducer_and_bus(&manifest, &bursts);
    assert_eq!(
        seal.get("status").and_then(Value::as_str),
        Some("transcript_sealed")
    );

    let first = bursts[0];
    let occurrence = LedgerOccurrenceIdentity::new(
        SESSION_ID,
        CAPTURE_EPOCH,
        first.sample_start,
        first.sample_end,
    );
    let formatter =
        ObservationIdentity::new(ObservationProducer::Formatter, 8_001, 1, occurrence.clone());
    let automatic = ledger.admit(&formatter, "Ivo");
    assert!(
        matches!(
            automatic,
            MutationReceipt::Refuse {
                reason: RefuseReason::SealedReplay,
                ..
            }
        ),
        "P0-B RED — missing ledger seal predicate/path: terminal Bus seal did not fence automatic formatter mutation; got {automatic:?}"
    );

    let manual = ObservationIdentity::new(ObservationProducer::ManualHuman, 9_001, 1, occurrence);
    assert!(
        ledger.admit(&manual, "Iwo").grants_mutation(),
        "explicit manual provenance remains the only valid post-seal supersession"
    );
}
