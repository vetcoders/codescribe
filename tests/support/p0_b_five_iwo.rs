//! Frozen P0-B five-Iwo conservation oracle.
//!
//! This module is compiled as a child of `presentation::transcript_bus`, so it
//! can drive the real reducer and Bus without widening the production API.  It
//! contains assertions and fixture interpretation only; occurrence admission,
//! reduction, Bus serialization and mutation decisions stay production-owned.

use super::*;
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, MutationReceipt, ObservationIdentity,
    ObservationProducer, OccurrenceIdentity as LedgerOccurrenceIdentity, RefuseReason,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST_REL: &str = "tests/fixtures/p0_b_five_iwo_manifest.json";
const EXPECTED_SCHEMA: &str = "codescribe.p0-b.five-iwo-fixture.v1";
const BUS_EVIDENCE_SCHEMA: &str = "codescribe.transcript-evidence.v1";
const SESSION_ID: &str = "p0-b-five-iwo";
const CAPTURE_EPOCH: u64 = 1;

#[derive(Clone, Debug)]
struct PublishedBusTrace {
    sealed_bytes: String,
    post_automatic_attempt_bytes: String,
}

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

fn fixture_calibration(manifest: &FixtureManifest) -> EnergyCalibration {
    EnergyCalibration::new(
        manifest
            .bursts
            .first()
            .expect("fixture burst")
            .evidence_calibration_version
            .clone(),
        manifest.minimum_energy_integral,
        manifest.minimum_valley_samples,
    )
}

fn fixture_evidence(burst: &FixtureBurst) -> AcousticEvidence {
    AcousticEvidence {
        occurrence: LedgerOccurrenceIdentity::new(
            SESSION_ID,
            CAPTURE_EPOCH,
            burst.sample_start,
            burst.sample_end,
        ),
        duration_ms: burst.duration_ms,
        energy_integral: burst.energy_integral,
        mean_rms_dbfs: burst.mean_rms_dbfs,
        peak_dbfs: burst.peak_dbfs,
        vad_open_sample: Some(burst.vad_open_sample),
        vad_close_sample: Some(burst.vad_close_sample),
        evidence_calibration_version: burst.evidence_calibration_version.clone(),
    }
}

fn admit_fixture_to_ledger(
    manifest: &FixtureManifest,
    bursts: &[&FixtureBurst],
) -> (AcousticLedger, Vec<(ObservationIdentity, MutationReceipt)>) {
    let mut ledger = AcousticLedger::new();
    let calibration = fixture_calibration(manifest);
    let mut admitted = Vec::with_capacity(bursts.len());
    for burst in bursts {
        let evidence = fixture_evidence(burst);
        let occurrence = evidence.occurrence.clone();
        assert!(
            ledger.qualify(&evidence, &calibration).is_qualified(),
            "burst {} must qualify from measured PCM evidence",
            burst.ordinal,
        );
        ledger.schedule_frontier(
            occurrence.clone(),
            vec![ObservationProducer::Apple, ObservationProducer::Whisper],
        );
        let apple = ObservationIdentity::new(
            ObservationProducer::Apple,
            burst.ordinal as u64,
            0,
            occurrence.clone(),
        );
        let apple_receipt = ledger.admit(&apple, &manifest.label);
        assert!(
            apple_receipt.is_insert(),
            "Apple must insert physical occurrence {}",
            burst.ordinal
        );
        assert!(!ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        admitted.push((apple, apple_receipt));
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
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper));
    }
    (ledger, admitted)
}

fn publish_through_reducer_and_bus(
    manifest: &FixtureManifest,
    bursts: &[&FixtureBurst],
) -> (String, PublishedBusTrace) {
    use super::super::emitter::TranscriptReducer;

    let (mut ledger, admitted) = admit_fixture_to_ledger(manifest, bursts);
    let occurrences = ledger.occurrences().cloned().collect::<Vec<_>>();
    for occurrence in &occurrences {
        ledger
            .seal(occurrence)
            .expect("closed qualified occurrence must seal");
    }
    let terminal = ledger
        .seal_terminal(SESSION_ID, CAPTURE_EPOCH)
        .expect("five-Iwo epoch must terminal-seal");

    let temp = tempfile::tempdir().expect("temporary Bus directory");
    let path = temp.path().join("events.jsonl");
    let bus = TranscriptBus::open_at(
        TranscriptSession {
            session_id: SESSION_ID.to_string(),
            mode: TranscriptMode::Dictation,
            has_latched_target: false,
            latched_target_is_self: false,
        },
        path.clone(),
        Some(manifest.sample_rate),
    )
    .expect("open synthetic Transcript Bus");
    bus.publish_started();

    let mut reducer = TranscriptReducer::default();
    let mut last_automatic_revision = None;
    for (observation, receipt) in &admitted {
        last_automatic_revision = Some(
            reducer
                .apply_ledger_mutation(&ledger, observation, receipt)
                .expect("qualified ledger mutation must revise the document"),
        );
    }
    let terminal_revision = reducer
        .apply_ledger_seal(&terminal)
        .expect("terminal seal must revise the document");
    let _ = bus.publish_revision(&terminal_revision, &ledger);
    let rendered = terminal_revision.rendered_text.clone();
    let sealed_bytes = fs::read_to_string(&path).expect("read terminal Bus trace");

    let _ = bus.publish_revision(
        last_automatic_revision
            .as_ref()
            .expect("at least one automatic reducer revision"),
        &ledger,
    );
    let post_automatic_attempt_bytes =
        fs::read_to_string(path).expect("read Bus trace after automatic replay attempt");

    (
        rendered,
        PublishedBusTrace {
            sealed_bytes,
            post_automatic_attempt_bytes,
        },
    )
}

fn complete_oracle_trace(manifest: &FixtureManifest) -> PublishedBusTrace {
    let samples = load_pcm(manifest);
    let bursts = energy_qualified(&samples, manifest);
    publish_through_reducer_and_bus(manifest, &bursts).1
}

fn parse_bus_evidence(raw: &str) -> Result<Vec<TranscriptBusEvidenceEvent>, Vec<String>> {
    let mut failures = Vec::new();
    let mut events = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(error) => {
                failures.push(format!("Bus JSONL line {} is invalid: {error}", index + 1));
                continue;
            }
        };
        if value.get("schema").and_then(Value::as_str) != Some(BUS_EVIDENCE_SCHEMA) {
            continue;
        }
        match serde_json::from_value::<TranscriptBusEvidenceEvent>(value) {
            Ok(event) => events.push(event),
            Err(error) => failures.push(format!(
                "Bus evidence line {} does not match {BUS_EVIDENCE_SCHEMA}: {error}",
                index + 1
            )),
        }
    }
    if failures.is_empty() {
        Ok(events)
    } else {
        Err(failures)
    }
}

fn evidence_jsonl(events: &[TranscriptBusEvidenceEvent]) -> String {
    let mut raw = events
        .iter()
        .map(|event| serde_json::to_string(event).expect("serialize Bus evidence mutant"))
        .collect::<Vec<_>>()
        .join("\n");
    if !raw.is_empty() {
        raw.push('\n');
    }
    raw
}

fn trace_from_evidence(events: &[TranscriptBusEvidenceEvent]) -> PublishedBusTrace {
    let raw = evidence_jsonl(events);
    PublishedBusTrace {
        sealed_bytes: raw.clone(),
        post_automatic_attempt_bytes: raw,
    }
}

fn validate_oracle_trace(trace: &PublishedBusTrace, expected: usize) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    let sealed_events = match parse_bus_evidence(&trace.sealed_bytes) {
        Ok(events) => events,
        Err(mut parse_failures) => {
            failures.append(&mut parse_failures);
            Vec::new()
        }
    };
    let post_attempt_events = match parse_bus_evidence(&trace.post_automatic_attempt_bytes) {
        Ok(events) => events,
        Err(mut parse_failures) => {
            failures.append(&mut parse_failures);
            Vec::new()
        }
    };
    if trace.sealed_bytes != trace.post_automatic_attempt_bytes {
        failures
            .push("Bus bytes changed after an automatic post-seal publication attempt".to_string());
    }

    let terminal_events = sealed_events
        .iter()
        .filter(|event| event.reducer_action == "record_ledger_terminal_seal")
        .collect::<Vec<_>>();
    if terminal_events.len() != expected {
        failures.push(format!(
            "five-Iwo conservation: expected {expected} committed words, observed {} (four-vs-five)",
            terminal_events.len()
        ));
    }
    if terminal_events.last().is_none_or(|event| {
        let rendered_words = event.rendered_text.split_whitespace().collect::<Vec<_>>();
        rendered_words.len() != expected
            || rendered_words
                .iter()
                .any(|word| !word.eq_ignore_ascii_case("Iwo"))
    }) {
        failures.push("Bus rendered_text does not conserve five Iwo labels".to_string());
    }
    if terminal_events
        .windows(2)
        .any(|pair| pair[0].rendered_text != pair[1].rendered_text)
    {
        failures.push("terminal Bus events disagree on rendered_text".to_string());
    }

    let mut terminal_receipts = BTreeSet::new();
    let mut previous_end = None;
    let mut previous_sequence = None;
    for (index, event) in terminal_events.iter().enumerate() {
        let ordinal = index + 1;
        if event.session_id != SESSION_ID || event.occurrence_session_id != SESSION_ID {
            failures.push(format!("word {ordinal} has the wrong Bus session identity"));
        }
        if event.capture_epoch != CAPTURE_EPOCH {
            failures.push(format!("word {ordinal} has the wrong capture epoch"));
        }
        if event.document_index != index as u64 {
            failures.push(format!("word {ordinal} has a non-canonical document index"));
        }
        if event.label != "Iwo" {
            failures.push(format!("word {ordinal} lost the Iwo label"));
        }
        if previous_sequence.is_some_and(|sequence| event.sequence <= sequence) {
            failures.push(format!("word {ordinal} has a replayed Bus sequence"));
        }
        previous_sequence = Some(event.sequence);
        if previous_end.is_some_and(|sample_end| event.sample_start < sample_end) {
            failures.push(format!("word {ordinal} overlaps an earlier PCM occurrence"));
        }
        previous_end = Some(event.sample_end);

        if event.acoustic_receipts.len() != 1 {
            failures.push(format!(
                "word {ordinal} must carry exactly one acoustic receipt"
            ));
            continue;
        }
        let receipt = &event.acoustic_receipts[0];
        if receipt.acoustic_serial_version == 0 {
            failures.push(format!(
                "word {ordinal} missing required evidence field acoustic_serial_version"
            ));
        }
        if receipt.acoustic_serial.is_empty() {
            failures.push(format!(
                "word {ordinal} missing required evidence field acoustic_serial"
            ));
        }
        if receipt.duration_ms == 0 {
            failures.push(format!(
                "word {ordinal} missing required evidence field duration_ms"
            ));
        }
        if receipt.energy_integral <= 0.0 {
            failures.push(format!("word {ordinal} lacks positive calibrated energy"));
        }
        if receipt.evidence_calibration_version.is_empty() {
            failures.push(format!(
                "word {ordinal} missing required evidence field evidence_calibration_version"
            ));
        }
        if receipt.sample_start != event.sample_start
            || receipt.sample_end != event.sample_end
            || receipt.vad_open_sample != event.sample_start
            || receipt.vad_close_sample != event.sample_end
        {
            failures.push(format!(
                "word {ordinal} lacks exact PCM/VAD coverage in Bus evidence"
            ));
        }
        let Some(seal_receipt) = receipt
            .seal_receipt
            .as_deref()
            .filter(|receipt| !receipt.is_empty())
        else {
            failures.push(format!(
                "word {ordinal} observation frontier is not proven closed by a Bus seal receipt"
            ));
            continue;
        };
        terminal_receipts.insert(seal_receipt);
        if receipt.word_evidence_receipts.is_empty() {
            failures.push(format!(
                "word {ordinal} missing retained-text word evidence receipt"
            ));
        }
        for required in ["apple-", "whisper-"] {
            if !receipt
                .layer_decision_receipts
                .iter()
                .any(|layer| layer.starts_with(required))
            {
                failures.push(format!(
                    "word {ordinal} missing {} layer decision",
                    required.trim_end_matches('-')
                ));
            }
        }
    }
    if terminal_events.len() == expected && terminal_receipts.len() != 1 {
        failures.push("missing one shared terminal ledger seal receipt in Bus bytes".to_string());
    }

    let terminal_sequence = terminal_events.iter().map(|event| event.sequence).max();
    if let Some(terminal_sequence) = terminal_sequence {
        for event in post_attempt_events
            .iter()
            .filter(|event| event.sequence > terminal_sequence)
        {
            if event.reducer_action != "apply_manual_edit" {
                failures.push(format!(
                    "accepted automatic post-seal mutation via Bus action {}",
                    event.reducer_action
                ));
                continue;
            }
            if event.acoustic_receipts.len() != 1
                || event.acoustic_receipts.iter().any(|receipt| {
                    receipt
                        .manual_edit_receipt
                        .as_deref()
                        .is_none_or(str::is_empty)
                })
            {
                failures.push("manual edit lacks provenance receipt".to_string());
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
        assert_eq!(burst.label, manifest.label);
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
    let (mut ledger, _) = admit_fixture_to_ledger(&manifest, &bursts);
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

    let mut no_energy_ledger = AcousticLedger::new();
    let mut no_energy = fixture_evidence(first);
    no_energy.energy_integral = 0.0;
    assert!(
        !no_energy_ledger
            .qualify(&no_energy, &fixture_calibration(&manifest))
            .is_qualified(),
        "no-energy control may not mint an acoustic serial"
    );
}

#[test]
fn p0_b_oracle_fails_closed_for_all_required_negative_controls() {
    let manifest = load_manifest();
    let complete = complete_oracle_trace(&manifest);
    assert!(
        validate_oracle_trace(&complete, 5).is_ok(),
        "complete oracle example must prove the verifier can pass"
    );
    let events = parse_bus_evidence(&complete.sealed_bytes).expect("production Bus evidence");
    assert_eq!(events.len(), 5);

    let four = trace_from_evidence(&events[..4]);
    let failures = validate_oracle_trace(&four, 5).unwrap_err().join("; ");
    assert!(failures.contains("four-vs-five"));

    let mut no_energy = events.clone();
    no_energy[0].acoustic_receipts[0].energy_integral = 0.0;
    let no_energy = trace_from_evidence(&no_energy);
    assert!(
        validate_oracle_trace(&no_energy, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("positive calibrated energy"))
    );

    let mut no_vad_close = events.clone();
    no_vad_close[0].acoustic_receipts[0].vad_close_sample = 0;
    let no_vad_close = trace_from_evidence(&no_vad_close);
    assert!(
        validate_oracle_trace(&no_vad_close, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("PCM/VAD coverage"))
    );

    let mut open_frontier = events.clone();
    open_frontier[0].acoustic_receipts[0].seal_receipt = None;
    let open_frontier = trace_from_evidence(&open_frontier);
    assert!(
        validate_oracle_trace(&open_frontier, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("frontier is not proven closed"))
    );

    let mut missing_serial = events.clone();
    missing_serial[0].acoustic_receipts[0]
        .acoustic_serial
        .clear();
    let missing_serial = trace_from_evidence(&missing_serial);
    assert!(
        validate_oracle_trace(&missing_serial, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("acoustic_serial"))
    );

    let mut missing_layer = events.clone();
    missing_layer[0].acoustic_receipts[0]
        .layer_decision_receipts
        .retain(|receipt| !receipt.starts_with("whisper-"));
    let missing_layer = trace_from_evidence(&missing_layer);
    assert!(
        validate_oracle_trace(&missing_layer, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("whisper layer decision"))
    );

    // N13: the validator consumes bytes emitted by production. Corrupting the
    // terminal reducer action in those bytes must turn the same trace RED.
    let mut corrupted_terminal_action = events.clone();
    corrupted_terminal_action[0].reducer_action = "apply_ledger_decision".to_string();
    let corrupted_terminal_action = trace_from_evidence(&corrupted_terminal_action);
    assert!(
        validate_oracle_trace(&corrupted_terminal_action, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("four-vs-five"))
    );

    let terminal_sequence = events
        .iter()
        .map(|event| event.sequence)
        .max()
        .expect("terminal Bus sequence");
    let mut automatic_event = events.last().expect("fifth Bus event").clone();
    automatic_event.sequence = terminal_sequence + 1;
    automatic_event.reducer_action = "apply_ledger_decision".to_string();
    automatic_event.acoustic_receipts[0].manual_edit_receipt = None;
    let mut post_automatic_events = events.clone();
    post_automatic_events.push(automatic_event);
    let mut automatic_mutation = complete.clone();
    automatic_mutation.post_automatic_attempt_bytes = evidence_jsonl(&post_automatic_events);
    assert!(
        validate_oracle_trace(&automatic_mutation, 5)
            .unwrap_err()
            .iter()
            .any(|failure| failure.contains("automatic post-seal mutation"))
    );

    let mut manual_event = events.last().expect("fifth Bus event").clone();
    manual_event.sequence = terminal_sequence + 1;
    manual_event.reducer_action = "apply_manual_edit".to_string();
    manual_event.acoustic_receipts[0].manual_edit_receipt = Some("manual-p0b-1".to_string());
    let mut manual_events = events;
    manual_events.push(manual_event);
    let manual_mutation = trace_from_evidence(&manual_events);
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

    let (ledger, _) = admit_fixture_to_ledger(&manifest, &bursts);
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
    let (mut ledger, _) = admit_fixture_to_ledger(&manifest, &bursts);
    let occurrences = ledger.occurrences().cloned().collect::<Vec<_>>();
    for occurrence in &occurrences {
        ledger
            .seal(occurrence)
            .expect("closed qualified occurrence must seal");
    }
    ledger
        .seal_terminal(SESSION_ID, CAPTURE_EPOCH)
        .expect("five-Iwo epoch must terminal-seal");
    let (_rendered, seal) = publish_through_reducer_and_bus(&manifest, &bursts);
    assert!(
        validate_oracle_trace(&seal, 5).is_ok(),
        "terminal Bus seal/frontier evidence must come from production JSONL"
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
