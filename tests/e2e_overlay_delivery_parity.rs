//! Overlay vs final-delivery parity on real fixture audio (mic-simulated).
//!
//! Feeds `tests/assets/data_assets/*.wav` through the **same** runtime path as
//! live recording (`collect_buffered_engine_events` → `transcription_session`,
//! ~100 ms callback chunks), then:
//!
//! 1. Builds **overlay assembly** = freezed `UtteranceFinal`s + open `Preview`
//!    (product contract: previous freezed, current interim appended).
//! 2. Builds **streaming floor** = joined finals only (what stop splice holds
//!    when previews are sealed).
//! 3. Runs **file final-pass** via `stt::transcribe_file_verdict` (same router
//!    as controller stop adjudication).
//! 4. Applies **length-regression floor** (`final_pass_is_length_regression`) —
//!    the delivery rule after the div0 Apple-file collapse.
//!
//! Opt-in (needs STT engine + model / Apple):
//! ```bash
//! CODESCRIBE_E2E_STT=1 cargo test --test e2e_overlay_delivery_parity -- --nocapture
//! # optional: force Candle for deterministic CI without Apple
//! CODESCRIBE_STT_ENGINE=candle CODESCRIBE_E2E_STT=1 cargo test --test e2e_overlay_delivery_parity -- --nocapture
//! # optional single clip:
//! CODESCRIBE_E2E_AUDIO=tests/assets/data_assets/01_no-to-dobra.wav ...
//! ```
//!
//! Always-on (no model): assembly contract + regression math on synthetic events.
//!
//! Authored-By: grok <agents@vetcoders.io>

use std::path::{Path, PathBuf};

use codescribe::audio;
use codescribe::presentation::emitter::reduce_transcript_events;
use codescribe_core::pipeline::contracts::{
    EngineEvent, FINAL_PASS_REGRESSION_MIN_STREAM_CHARS, LayerSource,
    final_pass_is_length_regression,
};
use codescribe_core::pipeline::streaming::{
    TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE, TailPatchDrainDisposition, TailPatchSessionReceipt,
    collect_buffered_engine_events,
};
use codescribe_core::quality::{MergeMode, merge_live_whisper};
use codescribe_core::stt;
use sha2::{Digest, Sha256};

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{STT_OPT_IN_ENV, skip_unless_opt_in};

// ── Product transcript reducer (shipped `PresentationEmitter`) ───────────────

fn overlay_assembly_from_events(events: &[EngineEvent]) -> String {
    reduce_transcript_events(events).rendered_text()
}

fn streaming_floor_from_events(events: &[EngineEvent]) -> String {
    reduce_transcript_events(events).streaming_floor()
}

// ── Lane guard: the parity bar must measure the lane it was told to measure ───

/// How many Layer 1 tail-patches actually reached the measured assembly.
///
/// Layer 1 is the ONLY producer of `ReplaceRange { source: TailPatch }`, so the
/// event stream is a witness for which lane ran. Reading the environment
/// instead would be unsound: the core injects `~/.codescribe/.env` into the
/// process env on the first `Config::load()`, which can land *after* the
/// session has already read the flag.
fn measured_tail_patch_count(events: &[EngineEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                EngineEvent::ReplaceRange {
                    source: LayerSource::TailPatch,
                    ..
                }
            )
        })
        .count()
}

/// Fail-closed contract between the target that asked for a lane and the lane
/// the run actually exercised.
///
/// Before the key was promoted to settings.json,
/// `Config::inject_file_env_for_runtime` copied it out of `~/.codescribe/.env`.
/// An operator who switched daily dictation to `phase1` therefore armed Layer 1
/// inside a target whose entire purpose is
/// to score Layer 0 against an Apple-fidelity reference — and Layer 1 is
/// *supposed* to diverge from Apple, so the bar went red for doing its job.
///
/// Measured 2026-08-08, one binary, consecutive runs of `make
/// test-engine-parity`: `Other: 1` → similarity 0.931 PASS, then `Other: 22` →
/// similarity 0.833 FAIL. The number was read as an engine regression; it was a
/// lane leak. This guard makes that leak an explicit failure with a name
/// instead of an unexplained score.
///
/// The mirror case matters just as much: a *layered* run that produced zero
/// patches scored Layer 0 while claiming to gate Layer 1 — the P1-01 shape (a
/// gate asserting nothing about the thing it exists to gate).
fn measured_lane_matches_request(
    requested_phase: Option<u8>,
    events: &[EngineEvent],
) -> Result<(), String> {
    let patches = measured_tail_patch_count(events);
    match (requested_phase, patches) {
        (None, 0) => Ok(()),
        (None, leaked) => Err(format!(
            "this target scores Layer 0 against the Apple-fidelity reference, but the run \
             measured Layer 1: {leaked} ReplaceRange{{TailPatch}} event(s) reached the \
             assembly. An unpinned compatibility override can arm the layer underneath a \
             Layer-0 bar. Pin the lane on the target \
             (`CODESCRIBE_LAYERED_TRANSCRIPTION=off`) and re-run — the similarity number \
             from this run says nothing about Layer 0."
        )),
        (Some(phase), 0) => Err(format!(
            "this target claims to gate Layer 1 (phase{phase}), but the run measured Layer 0: \
             zero ReplaceRange{{TailPatch}} events reached the assembly, so the similarity \
             number gates nothing about the layer (review P1-01). Check that the phase \
             reached the session and that the tail-patch lane is wired."
        )),
        (Some(_), _) => Ok(()),
    }
}

/// Product arming gate backed by the runtime receipt, not by settings/UI state
/// and not by the number of successful text mutations. A submitted window may
/// legitimately be skipped, but Layered ON with no submitted observation is a
/// failed run regardless of how clean the Apple transcript looked.
fn layered_arming_matches_request(
    requested: bool,
    receipt: Option<&TailPatchSessionReceipt>,
) -> Result<(), String> {
    if !requested {
        if receipt.is_some_and(|receipt| receipt.submitted > 0) {
            return Err("Layered OFF submitted Layer 1 windows".to_string());
        }
        return Ok(());
    }

    let receipt = receipt.ok_or_else(|| "Layered ON emitted no arming receipt".to_string())?;
    if !receipt.armed {
        return Err("Layered ON receipt says the lane was disarmed".to_string());
    }
    if receipt.submitted == 0 {
        return Err("Layered ON armed the lane but submitted zero windows".to_string());
    }
    Ok(())
}

fn sealed_final(utterance_id: u64, text: &str) -> EngineEvent {
    EngineEvent::UtteranceFinal {
        utterance_id,
        text: text.into(),
        raw_text: text.into(),
        start_ts: 0.0,
        end_ts: 1.0,
        segments: vec![],
        vad_speech_pct: None,
        avg_logprob: None,
        compression_ratio: None,
        quality_gate_dropped: false,
        confidence_flags: vec![],
    }
}

fn tail_patch(utterance_id: u64, start: usize, end: usize, text: &str) -> EngineEvent {
    EngineEvent::ReplaceRange {
        utterance_id,
        start,
        end,
        text: text.into(),
        source: LayerSource::TailPatch,
    }
}

fn layered_acceptance_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/layered_engine_acceptance.json"))
        .expect("canonical layered-engine acceptance fixture must stay valid JSON")
}

/// The refuted run, in miniature: `make test-engine-parity` (Layer 0) with the
/// operator's `.env` arming `phase1` underneath it.
#[test]
fn parity_lane_guard_catches_layer1_leaking_into_the_layer0_bar() {
    let leaked = vec![
        sealed_final(1, "korzystając z Tulczajn 2024"),
        tail_patch(1, 14, 22, "Toolchain"),
    ];
    let err = measured_lane_matches_request(None, &leaked)
        .expect_err("a Layer-0 bar that measured Layer 1 must not report a lane it never ran");
    assert!(
        err.contains("Layer 1"),
        "the failure must name the leaked lane, got: {err}"
    );
}

/// Mirror: the layered target must not silently score Layer 0.
#[test]
fn parity_lane_guard_catches_a_layered_run_that_never_armed_the_layer() {
    let unarmed = vec![sealed_final(1, "korzystając z Tulczajn 2024")];
    let err = measured_lane_matches_request(Some(1), &unarmed)
        .expect_err("a layered bar with zero tail-patches gates nothing (P1-01)");
    assert!(
        err.contains("Layer 0"),
        "the failure must name the lane actually measured, got: {err}"
    );
}

#[test]
fn parity_lane_guard_passes_when_the_measured_lane_is_the_requested_lane() {
    let layer0 = vec![sealed_final(1, "korzystając z Tulczajn 2024")];
    let layer1 = vec![
        sealed_final(1, "korzystając z Tulczajn 2024"),
        tail_patch(1, 14, 22, "Toolchain"),
    ];
    assert!(measured_lane_matches_request(None, &layer0).is_ok());
    assert!(measured_lane_matches_request(Some(1), &layer1).is_ok());
    // Seal-time lexicon (W1-A) also emits `ReplaceRange` and rides the Layer-0
    // lane by design — it must never be mistaken for a layered leak.
    let lexicon_on_layer0 = vec![
        sealed_final(1, "korzystając z Tulczajn 2024"),
        EngineEvent::ReplaceRange {
            utterance_id: 1,
            start: 14,
            end: 22,
            text: "Toolchain".into(),
            source: LayerSource::Lexicon,
        },
    ];
    assert!(measured_lane_matches_request(None, &lexicon_on_layer0).is_ok());
}

#[test]
fn layered_on_with_zero_submitted_windows_is_a_failed_arming_condition() {
    let receipt =
        TailPatchSessionReceipt::new(true, 0, 0, 0, 0, 0, TailPatchDrainDisposition::Completed);
    let err = layered_arming_matches_request(true, Some(&receipt))
        .expect_err("settings ON plus zero submitted work must fail closed");
    assert!(
        err.contains("zero windows"),
        "unexpected receipt gate: {err}"
    );
}

#[test]
fn layered_arming_uses_submitted_work_not_only_successful_mutations() {
    let honestly_skipped =
        TailPatchSessionReceipt::new(true, 1, 0, 1, 0, 0, TailPatchDrainDisposition::Completed);
    assert!(layered_arming_matches_request(true, Some(&honestly_skipped)).is_ok());

    let leaked_while_off =
        TailPatchSessionReceipt::new(false, 1, 0, 1, 0, 0, TailPatchDrainDisposition::NotArmed);
    assert!(layered_arming_matches_request(false, Some(&leaked_while_off)).is_err());
}

#[test]
fn stop_receipt_keeps_every_terminal_bucket_and_names_bounded_drain_timeout() {
    let events = vec![EngineEvent::Warning {
        code: TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE.to_string(),
        message:
            "armed=true submitted=10 applied=4 skipped=3 timed_out=1 abandoned=2 drain=timed_out"
                .to_string(),
    }];
    let receipt = TailPatchSessionReceipt::from_events(&events)
        .expect("runtime stop receipt must be recoverable from the event stream");

    assert_eq!(receipt.applied, 4);
    assert_eq!(receipt.skipped, 3);
    assert_eq!(receipt.timed_out, 1);
    assert_eq!(receipt.abandoned, 2);
    assert_eq!(
        receipt.applied + receipt.skipped + receipt.timed_out + receipt.abandoned,
        receipt.submitted,
        "stop evidence must account for every submitted window instead of silently passing"
    );
    assert_eq!(receipt.drain, TailPatchDrainDisposition::TimedOut);
}

/// Engine bar: multi-pause dictation must seal ≥2 freezed finals and cover the
/// spoken body (not a ~tens-of-chars tail on a 50s+ clip).
///
/// Missing words / Apple under-gen are **not** failure — they are the canvas for
/// Whisper fill + human + lexicon (Teacher). This bar only rejects "no engine":
/// single-final tail collapse, not imperfect WER.
fn assert_engine_multi_utterance_assembly(
    events: &[EngineEvent],
    human: Option<&str>,
    clip_label: &str,
) {
    let reducer = reduce_transcript_events(events);
    let sealed = reducer.committed_count();
    let full = reducer.rendered_text();
    let chars = full.chars().count();

    eprintln!(
        "  engine bar: sealed_finals={sealed} full_chars={chars}"
    );
    eprintln!("  ── live assembly (full) ──\n{full}\n  ── end live assembly ──");

    assert!(
        sealed >= 2,
        "CORE ENGINE: {clip_label} must emit ≥2 sealed UtteranceFinal (freezed+append); got {sealed}. \
         Single-final tail is not a dictation engine."
    );
    assert!(
        chars >= 120,
        "CORE ENGINE: {clip_label} live assembly too short ({chars} chars) — tail-only collapse"
    );

    if let Some(human) = human {
        // Per-lane doctrine (operator 2026-07-24, 85% thesis):
        // Apple live under-gen means **domain anchors live in gaps** — do NOT
        // require kubernetes/rust/etc on the live leg. Assert Polish body coverage
        // (char ratio) only. Domain anchors belong on **delivery after Whisper
        // fill**, checked separately when final-pass ran.
        let human_chars = human.chars().count().max(1);
        let ratio = chars as f32 / human_chars as f32;
        let pct = (ratio * 100.0).round() as i32;
        eprintln!("  engine bar live coverage vs human: {pct}% ({chars}/{human_chars})");
        assert!(
            ratio >= 0.20,
            "CORE ENGINE: live assembly covers only {pct}% of human for {clip_label} \
             (need ≥20% body coverage — gaps are fill canvas, not failure)"
        );
    }
}

#[test]
fn single_final_short_tail_fails_engine_bar() {
    // Pin the known broken shape so pseudo-success cannot regress.
    let events = vec![EngineEvent::UtteranceFinal {
        utterance_id: 1,
        text: "o Esterna przepisze krople".into(),
        raw_text: "o Esterna przepisze krople".into(),
        start_ts: 0.0,
        end_ts: 1.0,
        segments: vec![],
        vad_speech_pct: None,
        avg_logprob: None,
        compression_ratio: None,
        quality_gate_dropped: false,
        confidence_flags: vec![],
    }];
    let reducer = reduce_transcript_events(&events);
    assert_eq!(reducer.committed_count(), 1);
    assert!(reducer.rendered_text().chars().count() < 40);
    // Engine bar would fail: sealed < 2 and chars < 120.
    assert!(
        reducer.committed_count() < 2 || reducer.rendered_text().chars().count() < 120,
        "broken shape must fail engine bar predicates"
    );
}

/// Delivery after adjudicate length guard — mirrors controller production path:
/// length regression keeps stream floor; otherwise `merge_live_whisper` (never
/// full-replace live with Whisper — audit F1).
fn delivery_from_stream_and_final(stream: &str, final_text: &str) -> (&'static str, String) {
    let stream = stream.trim();
    let final_text = final_text.trim();
    if final_text.is_empty() {
        return ("streaming_floor", stream.to_string());
    }
    if final_pass_is_length_regression(final_text, stream) {
        return ("streaming_floor_after_regression", stream.to_string());
    }
    if stream.is_empty() {
        return ("final_pass", final_text.to_string());
    }
    let merged = merge_live_whisper(stream, final_text);
    let source = match merged.mode {
        MergeMode::Empty => "streaming_floor",
        MergeMode::LiveOnly => "streaming_floor",
        MergeMode::WhisperOnly => "final_pass",
        MergeMode::LiveFloorWhisperFill => "merged_live_whisper",
    };
    (source, merged.text)
}

/// Private STT fixtures live OUTSIDE the repo (real operator speech —
/// deprivatized twice). Resolution: `CODESCRIBE_DATA_ASSETS` →
/// `~/.codescribe/data_assets` → the gitignored in-repo drop dir.
fn data_assets_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CODESCRIBE_DATA_ASSETS") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        let local = PathBuf::from(home).join(".codescribe/data_assets");
        if local.is_dir() {
            return local;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/data_assets")
}

fn fixture_clips() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("CODESCRIBE_E2E_AUDIO") {
        let path = PathBuf::from(p);
        return vec![path];
    }
    // Shortest first for faster default opt-in runs.
    [
        "01_no-to-dobra.wav",
        "02_kubernetes-wymaga-konfiguracji.wav",
        "03_algorytm-ma-zlozonosc.wav",
        "04_runda-3-czyli.wav",
    ]
    .into_iter()
    .map(|name| data_assets_dir().join(name))
    .filter(|p| p.exists())
    .collect()
}

fn human_reference_for_wav(wav: &Path) -> Option<String> {
    let stem = wav.file_stem()?.to_str()?;
    let path = wav
        .parent()?
        .join(format!("{stem}_human_transcription.txt"));
    std::fs::read_to_string(path).ok()
}

/// Every WAV with a truth sibling, sorted for stable content-free recording IDs.
fn truth_paired_corpus() -> Vec<PathBuf> {
    let mut clips = std::fs::read_dir(data_assets_dir())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wav"))
        .filter(|path| human_reference_for_wav(path).is_some())
        .collect::<Vec<_>>();
    clips.sort();
    clips
}

fn edit_distance<T: Eq>(reference: &[T], hypothesis: &[T]) -> usize {
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    let mut current = vec![0; hypothesis.len() + 1];
    for (i, expected) in reference.iter().enumerate() {
        current[0] = i + 1;
        for (j, actual) in hypothesis.iter().enumerate() {
            current[j + 1] = if expected == actual {
                previous[j]
            } else {
                1 + previous[j].min(previous[j + 1]).min(current[j])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[hypothesis.len()]
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn word_error_rate(reference: &str, hypothesis: &str) -> f32 {
    let reference = normalized_words(reference);
    let hypothesis = normalized_words(hypothesis);
    edit_distance(&reference, &hypothesis) as f32 / reference.len().max(1) as f32
}

fn character_error_rate(reference: &str, hypothesis: &str) -> f32 {
    let reference = reference
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<Vec<_>>();
    let hypothesis = hypothesis
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<Vec<_>>();
    edit_distance(&reference, &hypothesis) as f32 / reference.len().max(1) as f32
}

fn normalized_characters(text: &str) -> Vec<char> {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn normalized_character_metrics(reference: &str, delivered: &str) -> (usize, f64) {
    let reference = normalized_characters(reference);
    let delivered = normalized_characters(delivered);
    let distance = edit_distance(&reference, &delivered);
    let denominator = reference.len().max(delivered.len()).max(1);
    let parity = (1.0 - distance as f64 / denominator as f64).clamp(0.0, 1.0);
    (distance, parity)
}

fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read acceptance input for hashing");
    format!("{:x}", Sha256::digest(bytes))
}

const REPAIR_WAVE_ROW_FIELDS: [&str; 21] = [
    "opaque_id",
    "class",
    "audio_sha256",
    "reference_sha256",
    "duration_seconds",
    "normalized_reference_chars",
    "normalized_delivered_chars",
    "normalized_edit_distance",
    "normalized_character_parity",
    "live_committed_chars",
    "committed_density_chars_per_second",
    "density_floor_chars_per_second",
    "density_floor_applies",
    "repeated_final_id_count",
    "overlapping_final_window_count",
    "head_present",
    "tail_present",
    "final_pass_attempted",
    "final_pass_skipped",
    "final_pass_skip_reason",
    "acceptance",
];

fn assert_repair_wave_report_contract(report: &serde_json::Value) {
    assert_eq!(report["schema"], "codescribe.repair-wave-acceptance.v1");
    let rows = report["recordings"]
        .as_array()
        .expect("acceptance recordings array");
    for row in rows {
        for field in REPAIR_WAVE_ROW_FIELDS {
            assert!(row.get(field).is_some(), "acceptance row missing {field}");
        }
    }
    for field in [
        "completed_recordings",
        "long_pass_count",
        "short_clean_count",
        "all_inputs_hash_verified",
        "all_acceptance_passed",
    ] {
        assert!(
            report["aggregate"].get(field).is_some(),
            "acceptance aggregate missing {field}"
        );
    }
    for forbidden in [
        "\"transcript\":",
        "\"reference_text\":",
        "\"delivered_text\":",
        "\"source_path\":",
        "\"source_basename\":",
        "\"patient_name\":",
        "\"client_name\":",
        "\"credential\":",
    ] {
        assert!(
            !report.to_string().contains(forbidden),
            "acceptance evidence contains forbidden key {forbidden}"
        );
    }
    if let Some(privacy) = report.get("privacy") {
        for field in [
            "transcript_bodies_written",
            "source_basenames_written",
            "private_absolute_paths_written",
            "patient_or_client_names_written",
            "credentials_written",
        ] {
            assert_eq!(
                privacy[field], false,
                "privacy verdict {field} must be false"
            );
        }
    }
}

#[test]
fn repair_wave_normalization_and_parity_follow_the_manifest_formula() {
    assert_eq!(normalized_characters(" A\nĄ\tB "), vec!['a', 'ą', 'b']);
    assert_eq!(normalized_character_metrics("Kot", "kot"), (0, 1.0));
    assert_eq!(normalized_character_metrics("abcd", "abxd"), (1, 0.75));
    assert_eq!(normalized_character_metrics("", "x"), (1, 0.0));
}

#[test]
fn repair_wave_schema_and_privacy_contract_reject_content_bearing_keys() {
    let row = REPAIR_WAVE_ROW_FIELDS
        .into_iter()
        .map(|field| (field.to_string(), serde_json::Value::Null))
        .collect::<serde_json::Map<_, _>>();
    let report = serde_json::json!({
        "schema": "codescribe.repair-wave-acceptance.v1",
        "aggregate": {
            "completed_recordings": 1,
            "long_pass_count": 0,
            "short_clean_count": 1,
            "all_inputs_hash_verified": true,
            "all_acceptance_passed": true,
        },
        "recordings": [row],
    });
    assert_repair_wave_report_contract(&report);
}

#[test]
#[should_panic(expected = "forbidden key")]
fn repair_wave_privacy_contract_fails_on_transcript_body() {
    let row = REPAIR_WAVE_ROW_FIELDS
        .into_iter()
        .map(|field| (field.to_string(), serde_json::Value::Null))
        .chain([(
            "delivered_text".to_string(),
            serde_json::Value::String("private content sentinel".to_string()),
        )])
        .collect::<serde_json::Map<_, _>>();
    let report = serde_json::json!({
        "schema": "codescribe.repair-wave-acceptance.v1",
        "aggregate": {
            "completed_recordings": 1,
            "long_pass_count": 0,
            "short_clean_count": 1,
            "all_inputs_hash_verified": true,
            "all_acceptance_passed": true,
        },
        "recordings": [row],
    });
    assert_repair_wave_report_contract(&report);
}

// ── Always-on contract tests (no STT / no model) ────────────────────────────

#[test]
fn overlay_assembly_freezes_finals_and_appends_preview_tail() {
    let events = vec![
        EngineEvent::Preview {
            rev: 1,
            text: "pierwsze".into(),
        },
        EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "pierwsze zdanie".into(),
            raw_text: "pierwsze zdanie".into(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: vec![],
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: vec![],
        },
        EngineEvent::Preview {
            rev: 2,
            text: "drugie".into(),
        },
        EngineEvent::Preview {
            rev: 3,
            text: "drugie zdanie live".into(),
        },
    ];

    let overlay = overlay_assembly_from_events(&events);
    assert_eq!(overlay, "pierwsze zdanie drugie zdanie live");

    let floor = streaming_floor_from_events(&events);
    assert_eq!(floor, "pierwsze zdanie");

    let reducer = reduce_transcript_events(&events);
    assert_eq!(reducer.committed_count(), 1);
    assert_eq!(reducer.streaming_floor(), "pierwsze zdanie");
    assert_eq!(reducer.rendered_text(), "pierwsze zdanie drugie zdanie live");
}

/// The parity bar measures `overlay_assembly_from_events` / the streaming
/// floor. With `CODESCRIBE_LAYERED_TRANSCRIPTION=phase1` the run also emits
/// Layer 1 `ReplaceRange { source: TailPatch }`, so those patches MUST be
/// visible in what the bar reads — otherwise the layered-on run scores exactly
/// like layered-off and the gate is blind to the layer it exists to gate.
///
/// Always-on (no model, no loopback): this is a contract on the harness itself,
/// and it is the reason the layered-on parity number can be trusted at all.
#[test]
fn parity_assembly_reads_layer1_tail_patches() {
    use codescribe_core::pipeline::contracts::LayerSource;

    let sealed = |id: u64, text: &str| EngineEvent::UtteranceFinal {
        utterance_id: id,
        text: text.into(),
        raw_text: text.into(),
        start_ts: 0.0,
        end_ts: 1.0,
        segments: vec![],
        vad_speech_pct: None,
        avg_logprob: None,
        compression_ratio: None,
        quality_gate_dropped: false,
        confidence_flags: vec![],
    };

    // Layer 0 under-generated "Toolchain" as "Tulczajn"; Layer 1 re-transcribed
    // the sealed window and patches the span in place.
    let layer0 = vec![sealed(1, "korzystając z Tulczajn 2024")];
    let layered = vec![
        sealed(1, "korzystając z Tulczajn 2024"),
        EngineEvent::ReplaceRange {
            utterance_id: 1,
            start: 14,
            end: 22,
            text: "Toolchain".into(),
            source: LayerSource::TailPatch,
        },
    ];

    let off = overlay_assembly_from_events(&layer0);
    let on = overlay_assembly_from_events(&layered);
    assert_eq!(off, "korzystając z Tulczajn 2024");
    assert_eq!(on, "korzystając z Toolchain 2024");
    assert_ne!(
        off, on,
        "layered-on parity must differ from layered-off — an identical score \
         means the bar never saw Layer 1"
    );
    // The floor (finals only, what stop splice holds) carries the patch too.
    assert_eq!(
        streaming_floor_from_events(&layered),
        "korzystając z Toolchain 2024"
    );
}

/// Canonical Kora acceptance: Apple may publish the weak hypothesis first, but
/// Layer 1 must repair the same committed span before the delivery boundary.
/// An empty `final_text` deliberately models `Final pass = off`; the live
/// tail-patch remains authoritative and does not require a whole-file repass.
#[test]
fn canonical_meaning_loss_is_repaired_live_before_delivery_with_final_pass_off() {
    let fixture = layered_acceptance_fixture();
    let weak_apple = fixture["meaning_loss"]["apple"]
        .as_str()
        .expect("meaning-loss Apple hypothesis");
    let spoken = fixture["meaning_loss"]["spoken"]
        .as_str()
        .expect("meaning-loss spoken truth");

    let events = vec![
        sealed_final(41, weak_apple),
        tail_patch(41, 0, weak_apple.chars().count(), spoken),
    ];

    assert_eq!(
        measured_tail_patch_count(&events),
        1,
        "the acceptance vector must exercise Layer 1 rather than blessing Apple-only text"
    );
    let repaired_live = streaming_floor_from_events(&events);
    assert_eq!(repaired_live, spoken);

    let (source, delivered) = delivery_from_stream_and_final(&repaired_live, "");
    assert_eq!(source, "streaming_floor");
    assert_eq!(
        delivered, spoken,
        "manual Retranscribe/full-file final must not be the first place meaning returns"
    );
    assert!(!delivered.contains("Musi latać"));
}

/// The first audible token is capture truth. A later in-span correction may
/// improve the body, but it may not eat the onset before seal or Delivery.
#[test]
fn onset_token_iwo_survives_live_patch_seal_and_delivery() {
    let fixture = layered_acceptance_fixture();
    let apple = fixture["onset"]["apple"]
        .as_str()
        .expect("onset Apple text");
    let repaired = fixture["onset"]["repaired"]
        .as_str()
        .expect("onset repaired text");
    let first_token = fixture["onset"]["required_first_token"]
        .as_str()
        .expect("required onset token");

    let events = vec![
        sealed_final(7, apple),
        tail_patch(7, 0, apple.chars().count(), repaired),
    ];
    let live = streaming_floor_from_events(&events);
    let (_, delivered) = delivery_from_stream_and_final(&live, "");

    assert!(
        live.starts_with(&format!("{first_token} ")),
        "live onset was lost: {live}"
    );
    assert!(
        delivered.starts_with(&format!("{first_token} ")),
        "delivery onset was lost after a valid live repair: {delivered}"
    );
}

/// Idempotence is keyed by span identity, never by equal text. Replaying the
/// same `utterance_id` updates its one slot; five new identities carrying the
/// same intentional phrase must remain five committed spans.
#[test]
fn same_span_replay_is_idempotent_while_five_distinct_repetitions_survive() {
    let fixture = layered_acceptance_fixture();
    let phrase = fixture["repetition"]["text"]
        .as_str()
        .expect("repetition text");
    let distinct_ids = fixture["repetition"]["distinct_span_ids"]
        .as_array()
        .expect("distinct span ids");
    let replayed_id = fixture["repetition"]["replayed_span_id"]
        .as_u64()
        .expect("replayed span id");
    let expected = fixture["repetition"]["expected_deliveries"]
        .as_u64()
        .expect("expected repetition count") as usize;
    let mut events = Vec::new();
    for utterance_id in distinct_ids {
        events.push(sealed_final(
            utterance_id.as_u64().expect("numeric span id"),
            phrase,
        ));
    }
    // Provider/session replay of span 3: same identity, therefore one slot.
    events.push(sealed_final(replayed_id, phrase));

    let reducer = reduce_transcript_events(&events);
    assert_eq!(reducer.committed_count(), expected);
    assert_eq!(
        reducer
            .streaming_floor()
            .split_whitespace()
            .filter(|token| *token == phrase)
            .count(),
        expected,
        "text-equality dedup must not erase intentional repetitions, while an identity replay must not create a sixth"
    );
}

#[test]
fn delivery_merges_live_floor_with_whisper_fill_not_full_replace() {
    // Same shape as teacher merge unit tests: live under-gen + whisper excess.
    let live = "korzystając z surowej transkrypcji Toolchain 2024";
    let whisper =
        "No to dobra teraz generalnie korzystając z surowej transkrypcji Tooltrain 2024 Dziękuję";
    let (source, delivery) = delivery_from_stream_and_final(live, whisper);
    assert_eq!(source, "merged_live_whisper");
    // Live floor: Toolchain must not be silently full-replaced by Tooltrain alone.
    let lower = delivery.to_lowercase();
    assert!(
        lower.contains("toolchain") || lower.contains("tooltrain"),
        "merged lost domain token: {delivery}"
    );
    // Whisper gap-fill openers land in the delivery.
    assert!(
        lower.contains("dobra") || lower.contains("generalnie") || lower.contains("no"),
        "whisper fill missing in: {delivery}"
    );
    // Not a silent full-replace of live with whisper.
    assert_ne!(delivery.trim(), whisper.trim());
    assert_ne!(delivery.trim(), live.trim());
}

#[test]
fn delivery_prefers_stream_when_file_final_collapses_like_div0_apple() {
    // Reference numbers from div0 2026-07-23 172031 session (not the asset itself):
    // ~98s audio, stream ~56 chars, Apple file final 12 chars "Im wystarczy".
    let stream = "Im wystarczy i jeszcze sporo z freezed live assembly utterance dwa trzy";
    let short_final = "Im wystarczy";
    assert!(stream.chars().count() >= FINAL_PASS_REGRESSION_MIN_STREAM_CHARS);
    assert!(final_pass_is_length_regression(short_final, stream));

    let (source, delivery) = delivery_from_stream_and_final(stream, short_final);
    assert_eq!(source, "streaming_floor_after_regression");
    assert_eq!(delivery, stream);
}

#[test]
fn data_assets_wav_fixtures_exist() {
    let clips = fixture_clips();
    if clips.is_empty() {
        // Clean public checkout: private fixtures are local-only by design.
        eprintln!(
            "Skipping: no local STT fixtures (set CODESCRIBE_DATA_ASSETS or \
             populate ~/.codescribe/data_assets — see tests/assets/data_assets/README.md)"
        );
        return;
    }
    for clip in &clips {
        assert!(clip.exists(), "missing fixture {}", clip.display());
        let (samples, rate) =
            audio::load_audio_file(clip).unwrap_or_else(|e| panic!("load {}: {e}", clip.display()));
        assert!(!samples.is_empty());
        assert!(rate >= 8_000);
        let secs = samples.len() as f32 / rate as f32;
        eprintln!(
            "fixture {} — {:.1}s @ {} Hz ({} samples)",
            clip.file_name().unwrap().to_string_lossy(),
            secs,
            rate,
            samples.len()
        );
    }
}

// ── Opt-in: real STT through live session + file final ──────────────────────

fn init_e2e_tracing() {
    // Without a subscriber, RUST_LOG is a no-op → minutes of silence on Apple live.
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,codescribe_core=info,codescribe=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .try_init();
}

#[tokio::test]
async fn e2e_file_audio_as_mic_overlay_and_delivery_parity() {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "overlay/delivery parity E2E",
        "Feeds data_assets WAVs as mic chunks through transcription_session, \
         then file final-pass + length floor (same rules as stop adjudicate).",
    ) {
        return;
    }

    init_e2e_tracing();

    let language = std::env::var("CODESCRIBE_E2E_LANG")
        .ok()
        .or_else(|| Some("pl".to_string()));

    let clips = fixture_clips();
    assert!(!clips.is_empty(), "no fixture clips");

    // Single clip by default for runtime; set CODESCRIBE_E2E_ALL_CLIPS=1 for matrix.
    let run_all = std::env::var("CODESCRIBE_E2E_ALL_CLIPS")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let clips: Vec<_> = if run_all {
        clips
    } else {
        clips.into_iter().take(1).collect()
    };

    for clip in clips {
        run_one_clip(&clip, language.clone()).await;
    }
}

async fn run_one_clip(clip: &Path, language: Option<String>) {
    eprintln!("═══════════════════════════════════════════════════════════");
    eprintln!("  Overlay/delivery parity — {}", clip.display());
    eprintln!("═══════════════════════════════════════════════════════════");

    let (samples, sample_rate) =
        audio::load_audio_file(clip).unwrap_or_else(|e| panic!("load {}: {e}", clip.display()));
    let duration = samples.len() as f32 / sample_rate as f32;
    eprintln!(
        "  audio: {:.1}s @ {} Hz, lang={:?}",
        duration, sample_rate, language
    );
    eprintln!(
        "  live session starting (Apple/Candle may take ~1–3× realtime; heartbeats every 10s)…"
    );
    let _ = std::io::Write::flush(&mut std::io::stderr());

    // 1) Live path — identical to StreamingRecorder → transcription_session.
    let t0 = std::time::Instant::now();
    let heartbeat = tokio::spawn(async move {
        let mut n = 0u64;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            n += 10;
            eprintln!("  … live session still running ({n}s wall elapsed)");
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
    });
    let events = collect_buffered_engine_events(&samples, sample_rate, language.clone())
        .await
        .unwrap_or_else(|e| panic!("live session on {}: {e}", clip.display()));
    heartbeat.abort();
    eprintln!("  live session done in {:?}", t0.elapsed());
    let _ = std::io::Write::flush(&mut std::io::stderr());

    let live = reduce_transcript_events(&events);
    let overlay = live.rendered_text();
    let stream_floor = live.streaming_floor();
    let preview_count = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::Preview { .. }))
        .count();
    let final_count = live.committed_count();

    eprintln!(
        "  events: total={} previews={} utterance_finals(sealed)={}",
        events.len(),
        preview_count,
        final_count
    );
    eprintln!(
        "  overlay_assembly chars={} (freezed+preview)",
        overlay.chars().count()
    );
    eprintln!(
        "  streaming_floor chars={} (finals only)",
        stream_floor.chars().count()
    );
    if !overlay.is_empty() {
        eprintln!("  ── overlay_assembly (full) ──\n{overlay}\n  ── end overlay_assembly ──");
    }
    if !stream_floor.is_empty() && stream_floor != overlay {
        eprintln!("  ── streaming_floor (full) ──\n{stream_floor}\n  ── end streaming_floor ──");
    }

    // Live must produce something for speech fixtures (otherwise STT cold/broken).
    assert!(
        !overlay.trim().is_empty() || !stream_floor.trim().is_empty(),
        "live assembly empty for speech fixture {} — check STT engine (CODESCRIBE_STT_ENGINE) \
         and model/Apple availability. events={}",
        clip.display(),
        events.len()
    );

    // Overlay freezed base must include sealed finals (append contract).
    if !stream_floor.is_empty() && !overlay.is_empty() {
        // Floor is freezed-only; overlay = floor + optional open tail.
        assert!(
            overlay.starts_with(stream_floor.trim())
                || overlay.contains(stream_floor.trim())
                || stream_floor
                    .split_whitespace()
                    .filter(|w| w.chars().count() > 3)
                    .take(3)
                    .all(|w| overlay.to_lowercase().contains(&w.to_lowercase())),
            "overlay assembly must include freezed finals.\noverlay={overlay}\nfloor={stream_floor}"
        );
    }

    // 2) File final-pass — same router as controller stop.
    let t1 = std::time::Instant::now();
    let final_verdict = stt::transcribe_file_verdict(clip, language.as_deref())
        .unwrap_or_else(|e| panic!("final-pass on {}: {e}", clip.display()));
    eprintln!(
        "  final-pass done in {:?} — engine={} mode={} chars={}",
        t1.elapsed(),
        final_verdict.engine.engine,
        final_verdict.engine.mode,
        final_verdict.text.chars().count()
    );
    eprintln!(
        "  ── final-pass text (full) ──\n{}\n  ── end final-pass ──",
        final_verdict.text
    );

    // 3) Delivery rule (stream floor of truth).
    let stream_for_floor = if !stream_floor.trim().is_empty() {
        stream_floor.as_str()
    } else {
        overlay.as_str()
    };
    let (source, delivery) =
        delivery_from_stream_and_final(stream_for_floor, final_verdict.text.as_str());
    eprintln!(
        "  delivery source={source} chars={}",
        delivery.chars().count()
    );
    eprintln!("  ── delivery (full) ──\n{delivery}\n  ── end delivery ──");

    if final_pass_is_length_regression(final_verdict.text.trim(), stream_for_floor) {
        assert_eq!(
            source, "streaming_floor_after_regression",
            "collapsing file final must not win over live assembly"
        );
        assert_eq!(delivery.trim(), stream_for_floor.trim());
        eprintln!("  ✓ length regression rejected (stream kept as floor)");
    } else if final_verdict.text.trim().is_empty() && !stream_for_floor.trim().is_empty() {
        // Empty Apple file final (common SFSpeechURL collapse) is also a
        // regression: delivery must keep the live/stream floor, never blank.
        assert_eq!(
            source, "streaming_floor",
            "empty file final must not replace non-empty live assembly"
        );
        assert_eq!(delivery.trim(), stream_for_floor.trim());
        eprintln!("  ✓ empty final rejected (stream kept as floor)");
    } else {
        assert!(
            matches!(
                source,
                "final_pass" | "streaming_floor" | "merged_live_whisper"
            ),
            "unexpected delivery source {source}"
        );
        eprintln!("  ✓ final accepted or stream used without collapse");
    }

    // Delivery must not be empty if we had speech content on either path.
    assert!(
        !delivery.trim().is_empty(),
        "delivery empty for {}",
        clip.display()
    );

    let human = human_reference_for_wav(clip);
    if let Some(ref human) = human {
        eprintln!("  ── human_reference (full) ──\n{human}\n  ── end human_reference ──");
        let human_lower = human.to_lowercase();
        let delivery_lower = delivery.to_lowercase();
        // Domain anchors on **delivery** (post Whisper fill / merge), not on Apple live.
        let domain_anchors = [
            "rust",
            "loctree",
            "codescribe",
            "kubernetes",
            "pacjent",
            "tokio",
            "toolchain",
            "lexicon",
            "leksykon",
        ];
        let human_hits: Vec<_> = domain_anchors
            .iter()
            .filter(|a| human_lower.contains(**a))
            .collect();
        if !human_hits.is_empty() {
            let matched = human_hits
                .iter()
                .filter(|a| delivery_lower.contains(**a))
                .count();
            eprintln!(
                "  delivery domain anchors: {matched}/{} ({human_hits:?})",
                human_hits.len()
            );
            assert!(
                matched >= 1,
                "delivery (after fill) shares no domain anchors with human for {}\n\
                 delivery={delivery}\n\
                 human_hits={human_hits:?}",
                clip.display()
            );
        }
    }

    // Core engine bar: multi-utterance freezed+append with body coverage.
    // Applies whenever live path ran (Apple or Candle); Apple is the hard product case.
    let clip_label = clip
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| clip.display().to_string());
    assert_engine_multi_utterance_assembly(&events, human.as_deref(), &clip_label);
}

// ═══════════════════════════════════════════════════════════════════════════
// Device-capture dictation: audio through REAL CoreAudio, not an mpsc shortcut
// ═══════════════════════════════════════════════════════════════════════════
//
// Every other test in this file feeds decoded samples straight into the
// session channel. This one goes the microphone's road: a player renders the
// fixture into a named loopback device (BlackHole), `StreamingRecorder`
// captures FROM that device via cpal — device selection, CoreAudio callbacks,
// resampling, ring buffer and backlog all engaged, at real time. The only
// test-specific element in the entire path is the device NAME in
// `AUDIO_INPUT_DEVICE` — the same production knob the app itself uses.
//
// Driven by scripts/e2e-blackhole-dictation.sh, which owns the environment
// preflight (device present, mic permission, system mixer not muted). The
// test owns the lifecycle: it arms capture BEFORE spawning the player, so the
// head of the fixture cannot be lost to compile/startup time — the exact
// regression (head-drop) this test exists to catch.

/// EngineEvent collector for the device-capture path (the session-internal
/// collector is private). Push-only; assertions read the final snapshot.
struct DeviceCaptureSink(std::sync::Mutex<Vec<EngineEvent>>);

impl codescribe_core::pipeline::contracts::EventSink for DeviceCaptureSink {
    fn on_event(&self, event: &EngineEvent) {
        self.0.lock().expect("collector lock").push(event.clone());
    }
}

/// Lowercased alphanumeric tokens of length ≥ 4 — long enough to be
/// discriminative in Polish, short enough to survive inflection variance.
fn coverage_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|t| t.chars().count() >= 4)
        .collect()
}

/// Full device-capture run: arm the recorder, play `clip` into `device` in
/// real time, return (events, assembled transcript, clip seconds). Shared by
/// the coverage test and the apple-live parity test — the CAPTURE ROAD must be
/// byte-identical between them or their verdicts measure different products.
async fn capture_clip_via_device(device: &str, clip: &Path) -> (Vec<EngineEvent>, String, f32) {
    let (samples, sample_rate) =
        audio::load_audio_file(clip).unwrap_or_else(|e| panic!("load {}: {e}", clip.display()));
    let clip_seconds = samples.len() as f32 / sample_rate as f32;
    eprintln!(
        "device-capture: {:.1}s fixture through '{device}' (engine: {})",
        clip_seconds,
        std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "default".into())
    );

    // 1) Arm capture FIRST — the recorder must already be listening when the
    //    first sample leaves the player, or the head of the fixture is lost.
    let sink = std::sync::Arc::new(DeviceCaptureSink(std::sync::Mutex::new(Vec::new())));
    let mut recorder = codescribe_core::audio::streaming_recorder::StreamingRecorder::new()
        .expect("recorder init (is the loopback device present?)");
    recorder.set_event_sink(Some(sink.clone()));
    recorder
        .start_event_session(Some("pl".to_string()))
        .await
        .expect("start capture session");

    // 2) Player renders the fixture into the loopback in real time. Its own
    //    self-diagnosis (device binding + rendered-peak) turns a silent render
    //    into a nonzero exit here instead of an empty-transcript mystery below.
    let player_script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/audio-play-to-device.swift");
    let mut player = std::process::Command::new("swift")
        .arg(&player_script)
        .arg(device)
        .arg(clip)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn loopback player");

    // 3) Real time is the contract: wait the clip out, plus swift-script
    //    startup (~3s) and a settle tail. A hard ceiling keeps a wedged player
    //    from hanging the suite.
    let deadline = std::time::Duration::from_secs_f32(clip_seconds + 25.0);
    let started = std::time::Instant::now();
    let player_status = loop {
        if let Some(status) = player.try_wait().expect("poll player") {
            break status;
        }
        if started.elapsed() > deadline {
            let _ = player.kill();
            panic!("player exceeded {deadline:?} for a {clip_seconds:.1}s clip");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    if !player_status.success() {
        let mut stderr_text = String::new();
        if let Some(mut pipe) = player.stderr.take() {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut stderr_text);
        }
        panic!("loopback player failed (setup problem, not an STT verdict): {stderr_text}");
    }

    // Let the tail of the audio drain through capture + session before stopping.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    recorder.stop().await.expect("stop capture session");

    let events = sink.0.lock().expect("collector lock").clone();
    let live = reduce_transcript_events(&events);
    let transcript = {
        let full = live.rendered_text();
        if full.trim().is_empty() {
            live.streaming_floor()
        } else {
            full
        }
    };
    eprintln!(
        "transcript chars={} sealed={} events={}",
        transcript.chars().count(),
        live.committed_count(),
        events.len()
    );
    // Event-shape census — when the transcript is empty, WHICH events arrived
    // is the entire diagnosis (silence looks like zero previews; a dead engine
    // looks like errors; a VAD miss looks like status-only traffic).
    {
        let mut census: std::collections::BTreeMap<&'static str, usize> =
            std::collections::BTreeMap::new();
        for event in &events {
            let name: &'static str = match event {
                EngineEvent::Preview { .. } => "Preview",
                EngineEvent::UtteranceFinal { .. } => "UtteranceFinal",
                _ => "Other",
            };
            *census.entry(name).or_default() += 1;
        }
        eprintln!("event census: {census:?}");
        for event in events.iter().take(20) {
            eprintln!("  event: {event:?}");
        }
    }
    eprintln!("── transcript ──\n{transcript}\n── end ──");
    (events, transcript, clip_seconds)
}

// `#[ignore]`, not an early `return`: a silent skip is reported by libtest as
// `ok`, so the doctrine bar looked green in every CI run while executing zero
// assertions (review P1-01). Ignored shows up as `ignored` in `test result:`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the BlackHole loopback — run via scripts/e2e-blackhole-dictation.sh"]
async fn e2e_device_capture_dictation() {
    assert_eq!(
        std::env::var("CODESCRIBE_E2E_CAPTURE_VIA_DEVICE")
            .ok()
            .as_deref(),
        Some("1"),
        "device-capture run needs CODESCRIBE_E2E_CAPTURE_VIA_DEVICE=1 \
         (run via scripts/e2e-blackhole-dictation.sh) — refusing to report a pass without capture"
    );
    let device = std::env::var("AUDIO_INPUT_DEVICE")
        .expect("device-capture run requires AUDIO_INPUT_DEVICE (the loopback device name)");

    let clip = std::env::var("CODESCRIBE_E2E_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets/data_assets/02_kubernetes-wymaga-konfiguracji.wav")
        });
    let reference_path = clip.with_file_name(format!(
        "{}_human_transcription.txt",
        clip.file_stem().unwrap().to_string_lossy()
    ));
    let reference = std::fs::read_to_string(&reference_path).unwrap_or_else(|e| {
        panic!(
            "device-capture verdict needs ground truth beside the fixture: {} ({e})",
            reference_path.display()
        )
    });

    let (_events, transcript, clip_seconds) = capture_clip_via_device(&device, &clip).await;

    assert!(
        !transcript.trim().is_empty(),
        "capture path produced NO text from {clip_seconds:.1}s of speech — \
         the loopback carried audio (player verified its render), so the loss \
         is in device selection, capture, or the session"
    );

    let reference_tokens: std::collections::BTreeSet<String> =
        coverage_tokens(&reference).into_iter().collect();
    let transcript_tokens: std::collections::BTreeSet<String> =
        coverage_tokens(&transcript).into_iter().collect();
    let hits = reference_tokens.intersection(&transcript_tokens).count();
    let coverage = hits as f32 / reference_tokens.len().max(1) as f32;
    eprintln!(
        "coverage {hits}/{} = {coverage:.2} (threshold 0.35)",
        reference_tokens.len()
    );
    // 0.35 is deliberately conservative: it tolerates engine variance and
    // loopback resampling, while still failing hard on the July field failure
    // shape (417s of speech → 234 chars ≈ coverage near zero).
    assert!(
        coverage >= 0.35,
        "transcript covers only {coverage:.2} of the reference vocabulary — \
         the capture path is losing or garbling audio"
    );

    // Head-drop regression: the OPENING of the fixture must be represented.
    let head_tokens: Vec<String> = coverage_tokens(&reference).into_iter().take(8).collect();
    let head_hit = head_tokens.iter().any(|t| transcript_tokens.contains(t));
    assert!(
        head_hit,
        "none of the first reference tokens {head_tokens:?} appear — \
         the head of the stream was dropped (the exact July regression)"
    );
}

// ─── The scoring instrument, written once ───────────────────────────────────

/// One token-similarity score from the Teacher aligner — the same instrument
/// the product uses for live↔whisper diffs, here diffing reference↔ours.
///
/// The metric is `equal / max(reference, ours)`, so text the reference does not
/// contain lowers the score by growing the **denominator**, not by breaking
/// matches. That is how one capture can score differently against two
/// references while matching the identical number of tokens: the ruler moved,
/// the capture did not. Any reading of a parity delta that ignores this reads
/// the wrong quantity.
///
/// Extracted because the parity run now reads it **twice** — once against the
/// Apple ruler (the gate) and once against the human ruler (the truth) — and
/// two copies of a metric are two metrics.
struct TeacherScore {
    equal: usize,
    reference_tokens: usize,
    our_tokens: usize,
    denominator: usize,
    similarity: f32,
    diff: String,
}

fn teacher_score(reference: &str, ours: &str) -> TeacherScore {
    use codescribe_core::quality::teacher::{AlignOp, align_words, tokenize};

    let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let target_tokens = tokenize(&normalize(reference));
    let our_tokens = tokenize(&normalize(ours));
    let ops = align_words(&target_tokens, &our_tokens);
    let equal = ops
        .iter()
        .filter(|op| matches!(op, AlignOp::Equal { .. }))
        .count();
    let denominator = target_tokens.len().max(our_tokens.len()).max(1);

    let mut diff = String::new();
    for op in &ops {
        match op {
            AlignOp::Equal { .. } => {}
            AlignOp::DeleteA { a } => {
                diff.push_str(&format!("  -ref  {}\n", target_tokens[*a].surface));
            }
            AlignOp::InsertB { b } => {
                diff.push_str(&format!("  +ours {}\n", our_tokens[*b].surface));
            }
            AlignOp::Substitute { a, b } => {
                diff.push_str(&format!(
                    "  ~     {} → {}\n",
                    target_tokens[*a].surface, our_tokens[*b].surface
                ));
            }
        }
    }

    TeacherScore {
        equal,
        reference_tokens: target_tokens.len(),
        our_tokens: our_tokens.len(),
        denominator,
        similarity: equal as f32 / denominator as f32,
        diff,
    }
}

/// Resolve the frozen system-engine transcript beside a fixture — the ruler the
/// 0.90 bar scores against. Sibling of `human_reference_for_wav`, which
/// resolves what was actually said.
fn apple_reference_for_wav(wav: &Path) -> Option<String> {
    let stem = wav.file_stem()?.to_str()?;
    let path = wav
        .parent()?
        .join(format!("{stem}_apple_live_reference.txt"));
    std::fs::read_to_string(path).ok()
}

/// Private-corpus quality proof through the production-owned replay cone.
///
/// Output is JSON containing counts and metrics only. Transcript bodies and
/// source filenames never leave process memory.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "private truth-paired corpus; set CODESCRIBE_PRODUCTION_CORPUS_OUT"]
async fn e2e_production_overlay_corpus_replay() {
    init_e2e_tracing();
    // Capture operator intent before `UserSettings::load` or a production
    // final-pass loader can re-seed process env from persisted settings. Each
    // recording restores these pins so one corpus run cannot silently blend
    // live lanes after the preceding recording's stop path.
    let requested_stt_engine = std::env::var("CODESCRIBE_STT_ENGINE").ok();
    let requested_layered = std::env::var("CODESCRIBE_LAYERED_TRANSCRIPTION").ok();
    let requested_local_final = std::env::var("CODESCRIBE_LOCAL_STT_FINAL_PASS").ok();
    let output_path = std::env::var("CODESCRIBE_PRODUCTION_CORPUS_OUT")
        .map(PathBuf::from)
        .expect("production corpus replay requires CODESCRIBE_PRODUCTION_CORPUS_OUT");
    let lane = match std::env::var("CODESCRIBE_PRODUCTION_REPLAY_LANE")
        .unwrap_or_else(|_| "apple_lexicon".to_string())
        .as_str()
    {
        "apple_lexicon" => {
            codescribe::controller::production_replay::ProductionReplayLane::AppleLexicon
        }
        "local_final_pass" => {
            codescribe::controller::production_replay::ProductionReplayLane::LocalFinalPass
        }
        other => panic!("unknown production replay lane {other}"),
    };
    let commit = std::env::var("CODESCRIBE_PRODUCTION_CORPUS_COMMIT")
        .expect("production corpus replay requires exact commit provenance");
    let clips = truth_paired_corpus();
    const EXPECTED_INPUTS: [(&str, &str, &str, &str); 3] = [
        (
            "tape-001",
            "long",
            "b4b395aa0f78293c5f5391eebdb76ae921084f7e02ae2fc641d8648b5a811408",
            "1b6133d5ca0f420b64549723a5d46cd08d7b33cb760a2c5f5f81074f4a8f7eb4",
        ),
        (
            "tape-002",
            "short",
            "2fa29b7d1797580651dfc48d4c33d2bf3a2a3072aff480af03512096acf6f360",
            "43592f8c852c76f9da36488ba8f12786179b46bf63ce0d59c995deaf90e17656",
        ),
        (
            "tape-003",
            "long",
            "eab63674877d0ccad08e8719119cf99ecbe9370811d8494e2156741548a48db1",
            "7f46925778ff93dec79c86de4305301c9ab4e5f46fb3e8021636174f843a03f6",
        ),
    ];
    assert_eq!(
        clips.len(),
        EXPECTED_INPUTS.len(),
        "exact repair-wave corpus must contain 3 truth-paired recordings"
    );

    let settings = codescribe_core::config::UserSettings::load();
    let resolved = settings.resolved_asr_mode();
    let language = std::env::var("CODESCRIBE_E2E_LANG")
        .ok()
        .or_else(|| Some("pl".to_string()));
    let started = std::time::Instant::now();
    let mut rows = Vec::with_capacity(clips.len());
    let mut all_inputs_hash_verified = true;

    for (index, clip) in clips.iter().enumerate() {
        // SAFETY: this ignored corpus test is the only selected test in its
        // process. The pins are restored before the production session starts,
        // before any session worker reads them.
        unsafe {
            if let Some(value) = requested_stt_engine.as_deref() {
                std::env::set_var("CODESCRIBE_STT_ENGINE", value);
            }
            if let Some(value) = requested_layered.as_deref() {
                std::env::set_var("CODESCRIBE_LAYERED_TRANSCRIPTION", value);
            }
            if let Some(value) = requested_local_final.as_deref() {
                std::env::set_var("CODESCRIBE_LOCAL_STT_FINAL_PASS", value);
            }
        }
        let (opaque_id, class, expected_audio_sha256, expected_reference_sha256) =
            EXPECTED_INPUTS[index];
        let reference_path = clip
            .parent()
            .expect("corpus recording parent")
            .join(format!(
                "{}_human_transcription.txt",
                clip.file_stem()
                    .expect("corpus recording stem")
                    .to_string_lossy()
            ));
        let audio_sha256 = sha256_file(clip);
        let reference_sha256 = sha256_file(&reference_path);
        let input_hashes_verified =
            audio_sha256 == expected_audio_sha256 && reference_sha256 == expected_reference_sha256;
        all_inputs_hash_verified &= input_hashes_verified;
        assert!(
            input_hashes_verified,
            "acceptance input hash mismatch for {opaque_id}"
        );
        let truth = std::fs::read_to_string(&reference_path).expect("read paired reference");
        let (samples, sample_rate) = audio::load_audio_file(clip)
            .unwrap_or_else(|error| panic!("load corpus recording {}: {error}", index + 1));
        let audio_seconds = samples.len() as f64 / f64::from(sample_rate);
        let run_started = std::time::Instant::now();
        let replay = codescribe::controller::production_replay::replay_overlay_recording(
            clip,
            language.clone(),
            &settings,
            lane,
        )
        .await
        .unwrap_or_else(|error| panic!("production replay recording {}: {error:#}", index + 1));

        let teacher = teacher_score(&truth, &replay.delivered_text);
        let wer = word_error_rate(&truth, &replay.delivered_text);
        let cer = character_error_rate(&truth, &replay.delivered_text);
        let truth_tokens = normalized_words(&truth);
        let delivered_tokens = normalized_words(&replay.delivered_text);
        let head = truth_tokens
            .iter()
            .take(8)
            .any(|token| delivered_tokens.contains(token));
        let tail = truth_tokens
            .iter()
            .rev()
            .take(8)
            .any(|token| delivered_tokens.contains(token));
        let token_ratio = delivered_tokens.len() as f64 / truth_tokens.len().max(1) as f64;
        let normalized_reference_chars = normalized_characters(&truth).len();
        let normalized_delivered_chars = normalized_characters(&replay.delivered_text).len();
        let (normalized_edit_distance, normalized_character_parity) =
            normalized_character_metrics(&truth, &replay.delivered_text);
        let live_committed_chars = replay.live_text.chars().count();
        let committed_density_chars_per_second = live_committed_chars as f64 / audio_seconds;
        let density_floor_chars_per_second = 4.0_f64;
        let density_floor_applies = audio_seconds > 10.0;
        let sealed = replay
            .events
            .iter()
            .filter(|event| matches!(event, EngineEvent::UtteranceFinal { .. }))
            .count();
        let previews = replay
            .events
            .iter()
            .filter(|event| matches!(event, EngineEvent::Preview { .. }))
            .count();
        let tail_patches = measured_tail_patch_count(&replay.events);
        let tail_patch_receipt = TailPatchSessionReceipt::from_events(&replay.events);
        if replay.layer1_armed {
            layered_arming_matches_request(true, tail_patch_receipt.as_ref()).unwrap_or_else(
                |mismatch| {
                    panic!("production replay {opaque_id} Layer 1 arming mismatch: {mismatch}")
                },
            );
        }

        let acceptance = if class == "long" {
            normalized_character_parity >= 0.85
                && head
                && tail
                && (committed_density_chars_per_second >= density_floor_chars_per_second
                    || !replay.final_pass_skipped)
        } else {
            replay.boundary_evidence.repeated_final_id_count == 0
                && replay.boundary_evidence.overlapping_final_window_count == 0
                && head
                && tail
        };
        let mut row = serde_json::json!({
            "opaque_id": opaque_id,
            "class": class,
            "audio_sha256": audio_sha256,
            "reference_sha256": reference_sha256,
            "duration_seconds": audio_seconds,
            "normalized_reference_chars": normalized_reference_chars,
            "normalized_delivered_chars": normalized_delivered_chars,
            "normalized_edit_distance": normalized_edit_distance,
            "normalized_character_parity": normalized_character_parity,
            "live_committed_chars": live_committed_chars,
            "committed_density_chars_per_second": committed_density_chars_per_second,
            "density_floor_chars_per_second": density_floor_chars_per_second,
            "density_floor_applies": density_floor_applies,
            "repeated_final_id_count": replay.boundary_evidence.repeated_final_id_count,
            "overlapping_final_window_count": replay.boundary_evidence.overlapping_final_window_count,
            "head_present": head,
            "tail_present": tail,
            "final_pass_attempted": replay.final_pass_attempted,
            "final_pass_skipped": replay.final_pass_skipped,
            "final_pass_skip_reason": replay.final_pass_skip_reason,
            "acceptance": acceptance,
            "input_hashes_verified": input_hashes_verified,
        });
        row.as_object_mut().expect("acceptance row object").extend(
            serde_json::json!({
                "sample_rate_hz": sample_rate,
                "lane": replay.lane.as_token(),
                "layer1_armed": replay.layer1_armed,
                "transcript_source": replay.transcript_source,
                "engine_label": replay.engine_label,
                "events": replay.events.len(),
                "previews": previews,
                "sealed_finals": sealed,
                "final_count": replay.boundary_evidence.final_count,
                "unique_final_id_count": replay.boundary_evidence.unique_final_id_count,
                "tail_patches": tail_patches,
                "tail_patch_submitted": tail_patch_receipt.as_ref().map(|receipt| receipt.submitted),
                "tail_patch_applied": tail_patch_receipt.as_ref().map(|receipt| receipt.applied),
                "tail_patch_skipped": tail_patch_receipt.as_ref().map(|receipt| receipt.skipped),
                "tail_patch_timed_out": tail_patch_receipt.as_ref().map(|receipt| receipt.timed_out),
                "tail_patch_abandoned": tail_patch_receipt.as_ref().map(|receipt| receipt.abandoned),
                "tail_patch_drain": tail_patch_receipt.as_ref().map(|receipt| format!("{:?}", receipt.drain)),
                "live_chars": replay.live_text.chars().count(),
                "adjudicated_chars": replay.adjudicated_text.chars().count(),
                "delivered_chars": replay.delivered_text.chars().count(),
                "truth_tokens": truth_tokens.len(),
                "delivered_tokens": delivered_tokens.len(),
                "token_ratio": token_ratio,
                "teacher_similarity": teacher.similarity,
                "wer": wer,
                "cer": cer,
                "wall_seconds": run_started.elapsed().as_secs_f64(),
            })
            .as_object()
            .expect("diagnostic row object")
            .clone(),
        );
        rows.push(row);
        eprintln!(
            "production corpus {}: lane={} parity={:.3} density={:.3} final_pass_skipped={} acceptance={}",
            opaque_id,
            lane.as_token(),
            normalized_character_parity,
            committed_density_chars_per_second,
            replay.final_pass_skipped,
            acceptance,
        );
    }

    let count = rows.len();
    let long_pass_count = rows
        .iter()
        .filter(|row| row["class"] == "long" && row["acceptance"] == true)
        .count();
    let short_clean_count = rows
        .iter()
        .filter(|row| row["class"] == "short" && row["acceptance"] == true)
        .count();
    let all_acceptance_passed = rows.iter().all(|row| row["acceptance"] == true);
    let report = serde_json::json!({
        "schema": "codescribe.repair-wave-acceptance.v1",
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "commit": commit,
        "configuration": {
            "replay_lane": lane.as_token(),
            "resolved_asr_mode": resolved.mode.as_str(),
            "gateway": "unavailable",
            "language": language,
            "stt_engine": requested_stt_engine.as_deref().unwrap_or("auto"),
            "layered_transcription": requested_layered.as_deref().unwrap_or("unset"),
            "local_final_pass": requested_local_final.as_deref().unwrap_or("unset"),
            "capture_pacing_ms": 100,
            "event_reducer": "presentation_emitter_transcript_reducer",
            "stop_adjudication": "production",
            "delivery_postprocess": "production",
        },
        "aggregate": {
            "completed_recordings": count,
            "long_pass_count": long_pass_count,
            "short_clean_count": short_clean_count,
            "all_inputs_hash_verified": all_inputs_hash_verified,
            "all_acceptance_passed": all_acceptance_passed,
            "wall_seconds": started.elapsed().as_secs_f64(),
        },
        "recordings": rows,
        "privacy": {
            "transcript_bodies_written": false,
            "source_basenames_written": false,
            "private_absolute_paths_written": false,
            "patient_or_client_names_written": false,
            "credentials_written": false,
        },
    });
    assert_repair_wave_report_contract(&report);
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).expect("create production corpus artifact directory");
    }
    std::fs::write(
        &output_path,
        serde_json::to_vec_pretty(&report).expect("serialize redacted corpus report"),
    )
    .expect("write redacted production corpus report");
    assert!(
        all_inputs_hash_verified,
        "not all exact input hashes verified"
    );
    assert!(
        all_acceptance_passed,
        "repair-wave acceptance failed; inspect redacted metrics"
    );
    eprintln!(
        "production corpus complete: lane={} recordings={} redacted_artifact_written=true",
        lane.as_token(),
        count
    );
}

/// The Apple reference is a **ruler, not the truth**: it is one engine's output
/// from the fixture, and `PARITY_SIMILARITY_BAR` scores our capture against it.
/// This measures the gap between that ruler and what was actually said — the
/// ceiling any Apple-scored arm can reach *while being faithful to the speech*.
///
/// Why this test exists at all: every human-reference number this plan reasons
/// from (`AGENTS.md`, layered-pipeline docs) was quoted from
/// `examples/apple_backend_bars.rs`, which no gate, script or CI job executes.
/// A doctrine resting on a number nothing re-derives is a doctrine resting on a
/// memory. This one needs no microphone, no loopback and no engine, so unlike
/// every other parity number here it is reproducible by any agent on any host
/// that has the fixtures.
///
/// It deliberately asserts **no quality bar** — inventing one is an operator
/// decision. It asserts only the fact the bar's readers keep losing: scoring
/// against Apple is provably not scoring against what was said.
#[test]
fn apple_reference_is_a_ruler_not_the_truth() {
    let clip = data_assets_dir().join("05_apple-live-parity.wav");
    let (Some(apple), Some(human)) = (
        apple_reference_for_wav(&clip),
        human_reference_for_wav(&clip),
    ) else {
        // A declared absence, not a silent pass: this line is the whole result
        // when the fenced fixtures are not on the host (they are the operator's
        // own speech and never enter the repo).
        eprintln!(
            "ruler-vs-truth: NO NUMBER — fixtures absent beside {} \
             (private assets: CODESCRIBE_DATA_ASSETS or ~/.codescribe/data_assets)",
            clip.display()
        );
        return;
    };

    let score = teacher_score(&apple, &human);
    eprintln!(
        "ruler-vs-truth: the Apple reference scores {}/{} = {:.3} against the human \
         transcription of the same audio ({} Apple-ruler tokens vs {} spoken tokens). \
         A capture that reproduced the SPEECH perfectly would score about this \
         much against the Apple ruler, not 1.000 — so the {:.2} bar is measured \
         in Apple-fidelity, never in accuracy.",
        score.equal,
        score.denominator,
        score.similarity,
        score.reference_tokens,
        score.our_tokens,
        0.90_f32,
    );

    assert!(
        !apple.trim().is_empty() && !human.trim().is_empty(),
        "both references must carry text — an empty ruler scores everything 0.000"
    );
    assert!(
        score.similarity > 0.0,
        "the two references share no tokens — they are not transcripts of the same audio, \
         and every number scored against this ruler is meaningless"
    );
    assert!(
        score.similarity < 1.0,
        "the Apple reference and the human transcription are token-identical, so the \
         layered doctrine's premise is false on this fixture: scoring against Apple \
         WOULD be scoring against the truth, and `AGENTS.md`'s 'never against Apple' \
         rule needs re-deriving rather than implementing"
    );
}

// ─── Reversed TDD: apple-live parity ────────────────────────────────────────
//
// The spec is not synthetic: `05_apple-live-parity.wav` is the operator's own
// speech, and `_apple_live_reference.txt` is VERBATIM what the SYSTEM Apple
// live dictation wrote from that same audio (see data_assets/README.md). Same
// neural engine, same audio — so anything short of the same text is OUR
// plumbing: per-request WAV windowing, seal head-drop, merge garbling.
//
// The bar is normalized EXACT MATCH (whitespace collapsed; case and
// punctuation kept — letter-level is the operator's bar). This test is RED by
// design until the streaming bridge v2 path reproduces the system engine's
// output; every run prints token similarity + a word-level diff so each
// grinding iteration has numbers, and the diff doubles as Teacher material
// (what our path loses/garbles against a same-engine reference).

// See `e2e_device_capture_dictation`: ignored rather than silently skipped, so
// the AGENTS.md doctrine bar can never be mistaken for a passing gate (P1-01).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the BlackHole loopback — run via `make test-engine-parity`"]
async fn e2e_apple_live_parity() {
    // Green bar for word-level similarity against the system-engine reference.
    // The value is FENCED — restating it is an operator decision
    // (.vibecrafted/plans/w12-layered-live-closure/reports/default-flip-memo-layered.md).
    //
    // What is NOT fenced is describing it honestly. This comment used to claim
    // the bar sat "under the measured floor of 4 identical runs (0.918–0.931)".
    // The wider Layer-0 sample (n=10, lane-leaked runs excluded) is
    // 0.778 / 0.898 / 0.909 ×3 / 0.920 / 0.924 ×2 / 0.931 ×2: two runs land
    // BELOW 0.90, so the bar is not under the floor and a single green run is a
    // sample, not proof. `AGENTS.md` carries the same sample; this comment used
    // to contradict it.
    const PARITY_SIMILARITY_BAR: f32 = 0.90;
    // Word-count ratio window against the Apple reference. Named (values
    // unchanged) so the headroom line below can print the same ceiling the
    // assertion enforces instead of restating 1.1 in two places that can drift.
    const RATIO_BAR_MIN: f32 = 0.9;
    const RATIO_BAR_MAX: f32 = 1.1;
    assert_eq!(
        std::env::var("CODESCRIBE_E2E_CAPTURE_VIA_DEVICE")
            .ok()
            .as_deref(),
        Some("1"),
        "parity run needs CODESCRIBE_E2E_CAPTURE_VIA_DEVICE=1 (run via `make test-engine-parity`) \
         — refusing to report a pass without capture"
    );
    let device = std::env::var("AUDIO_INPUT_DEVICE")
        .expect("parity run requires AUDIO_INPUT_DEVICE (the loopback device name)");

    // Snapshot the requested lane BEFORE the capture path can run
    // `Config::load()`: the first load injects ~/.codescribe/.env into the
    // process environment, so reading this afterwards would report a leak as if
    // it were the request. See `measured_lane_matches_request`.
    let requested_lane = codescribe_core::stt::tail_patcher::layered_phase();

    let clip = std::env::var("CODESCRIBE_E2E_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_assets_dir().join("05_apple-live-parity.wav"));
    let reference_path = clip.with_file_name(format!(
        "{}_apple_live_reference.txt",
        clip.file_stem().unwrap().to_string_lossy()
    ));
    let reference = std::fs::read_to_string(&reference_path).unwrap_or_else(|e| {
        panic!(
            "parity needs the system-engine reference beside the fixture: {} ({e})",
            reference_path.display()
        )
    });

    let (events, transcript, clip_seconds) = capture_clip_via_device(&device, &clip).await;
    assert!(
        !transcript.trim().is_empty(),
        "capture path produced NO text from {clip_seconds:.1}s of speech"
    );

    // Which lane did this run actually measure? Print it before any number, so
    // every retained log under target/e2e-blackhole/ is self-describing, and
    // fail loudly on a mismatch rather than reporting the wrong lane's score.
    eprintln!(
        "parity lane: requested {} · measured {} tail-patch event(s)",
        match requested_lane {
            None => "layer0 (off)".to_string(),
            Some(phase) => format!("layer1 (phase{phase})"),
        },
        measured_tail_patch_count(&events)
    );
    if let Err(mismatch) = measured_lane_matches_request(requested_lane, &events) {
        panic!("parity lane mismatch — {mismatch}");
    }
    let tail_patch_receipt = TailPatchSessionReceipt::from_events(&events);
    if let Err(mismatch) =
        layered_arming_matches_request(requested_lane.is_some(), tail_patch_receipt.as_ref())
    {
        panic!("parity Layer 1 arming mismatch — {mismatch}");
    }
    if let Some(receipt) = tail_patch_receipt {
        eprintln!(
            "parity Layer 1 receipt: armed={} submitted={} applied={} skipped={} timed_out={} abandoned={} drain={:?}",
            receipt.armed,
            receipt.submitted,
            receipt.applied,
            receipt.skipped,
            receipt.timed_out,
            receipt.abandoned,
            receipt.drain,
        );
    }

    let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let ours = normalize(&transcript);

    // ── Which ruler GATES this run — decided once, by the lane's job ─────────
    //
    // Layer 0's job is to reproduce Apple's live canvas, so Apple fidelity is
    // its ruler and every Apple-referenced bar below gates it.
    //
    // Layer 1's job is to DIVERGE from Apple toward what was actually said
    // (`AGENTS.md`: "a layered run must be scored against the human reference
    // beside the fixture, never against Apple"). Until this line existed, that
    // sentence was doctrine the code contradicted: both lanes ran through the
    // same Apple-referenced asserts, so the layered arm was marked FAIL for
    // doing its job — three independent measurements agreed on the sign
    // (similarity down, accuracy up) and the gate read only the first.
    //
    // So in the layered lane the Apple numbers are MEASURED and PRINTED, never
    // a verdict. What still gates there is lane-agnostic truth: the head is
    // present, the tail is sealed, no spans were lost, and the lane that ran is
    // the lane that was asked for.
    //
    // The obvious-looking alternative — gate the layered lane on
    // `accuracy-vs-human` instead — is rejected deliberately: that reference is
    // a PRIVATE fixture (`~/.codescribe/data_assets`, never in the repo), so
    // such a gate would evaporate silently on any tree without the operator's
    // corpus. A gate that disappears when its input is missing is the exact
    // failure this test already survived once. Accuracy stays informational
    // until its reference is something a checkout can see.
    let apple_rulers_gate = requested_lane.is_none();

    // Arm 1 — the Apple ruler: fidelity to the system engine.
    // Keep this line's shape (`^parity similarity <n>/<d> = <v>`): the
    // `test-engine-parity-both` extractor parses it.
    let apple = teacher_score(&reference, &ours);
    let (equal, similarity) = (apple.equal, apple.similarity);
    let (target_tokens, our_tokens) = (apple.reference_tokens, apple.our_tokens);
    eprintln!(
        "parity similarity {equal}/{} = {similarity:.3} ({})",
        apple.denominator,
        if apple_rulers_gate {
            format!("bar: >= {PARITY_SIMILARITY_BAR} — GATING in this lane")
        } else {
            format!(
                "bar {PARITY_SIMILARITY_BAR} NOT APPLIED — wrong instrument for the layer1 lane; \
                 measurement only"
            )
        }
    );

    // Arm 2 — the ACCURACY ruler: fidelity to what was actually said.
    //
    // `AGENTS.md` mandates that a layered run "must be scored against the human
    // reference beside the fixture, never against Apple", because gap-filling
    // grows the denominator against an Apple ruler and so a MORE accurate layer
    // scores LOWER. Until this arm existed, nothing in the tree implemented that
    // sentence: the only human-defaulting instrument was
    // `examples/apple_backend_bars.rs`, referenced by nothing but its own
    // doc-comment, so the doctrine's 0.897/0.800 numbers came from the file lane
    // — never from the live assembly under judgement.
    //
    // Printed for BOTH lanes and gating NEITHER — see `apple_rulers_gate`: the
    // human reference is a private fixture, so a bar on it would vanish
    // silently wherever the corpus is absent. It is kept as the axis on which
    // the operator's default-flip decision can be made with both numbers on the
    // table, and (below) as the only evidence that can excuse a ratio-ceiling
    // breach in the layered lane.
    let accuracy = human_reference_for_wav(&clip).map(|human| teacher_score(&human, &ours));
    match &accuracy {
        Some(acc) => {
            eprintln!(
                "parity accuracy-vs-human {}/{} = {:.3} (no bar — informational; \
                 see AGENTS.md 'never against Apple')",
                acc.equal, acc.denominator, acc.similarity
            );
            // The structural cliff, printed BEFORE the assert that enforces it.
            //
            // The ratio bar below is scored against the Apple ruler, so the
            // closer a layer gets to the SPOKEN token count, the closer it
            // drives that ratio to the hard 1.1 ceiling. Measured on this
            // fixture: Apple 171 tokens, spoken 195 — so a perfectly accurate
            // capture scores ratio 1.14 and trips a bar whose message blames
            // "duplicated phrases". Nobody was watching this number; it is a
            // harder gate on accuracy than the similarity bar everyone argues
            // about, because it has no nondeterminism margin.
            let ceiling = (RATIO_BAR_MAX * target_tokens as f32).floor() as usize;
            eprintln!(
                "parity accuracy-headroom: {our_tokens} of at most {ceiling} tokens allowed by \
                 the ratio bar ({} left); reaching the spoken {} would need {} more — \
                 accuracy runs out of ratio budget {}",
                ceiling.saturating_sub(our_tokens),
                acc.reference_tokens,
                acc.reference_tokens.saturating_sub(our_tokens),
                if acc.reference_tokens > ceiling {
                    "BEFORE it can be reached"
                } else {
                    "with room to spare"
                }
            );
        }
        None => eprintln!(
            "parity accuracy-vs-human: NO NUMBER — no human transcription beside {} \
             (the layered lane cannot be judged on accuracy from this run)",
            clip.display()
        ),
    }

    {
        let diff = apple.diff;
        if diff.is_empty() {
            eprintln!("word diff: none");
        } else {
            eprintln!("── word diff (ref → ours) ──\n{diff}── end diff ──");
        }
    }

    // Confrontation: what our pipeline produced from this same speech on the
    // night of the recording (machine-local evidence; absent elsewhere).
    if let Some(home) = std::env::var_os("HOME") {
        let that_night = Path::new(&home)
            .join(".codescribe/transcriptions/2026-07-25/015148_zrobic-teraz-zrobic_raw.txt");
        if let Ok(before) = std::fs::read_to_string(&that_night) {
            eprintln!(
                "── that night (015148, {} chars from 417s of speech) ──\n{}\n── end ──",
                before.chars().count(),
                before.trim()
            );
        }
    }

    // Structural bars are deterministic — they defend every fix of this wave
    // (windowing, duplicate phrases, lost spans, truncated tail) regardless of
    // engine noise. Word choice is NOT deterministic: 4 runs of identical code
    // on this fixture scored 0.918/0.919/0.925/0.931 with different word-level
    // errors each time (SFSpeech internal nondeterminism; signal level ruled
    // out by measurement — normalizing -25→-1 dBFS did not change the spread).
    let ours_lower = ours.to_lowercase();
    let head: String = ours_lower.chars().take(120).collect();
    assert!(
        head.contains("teraz zaczynam"),
        "head of the dictation is missing — the capture path dropped the fixture's opening"
    );
    let tail: String = {
        let chars: Vec<char> = ours_lower.chars().collect();
        chars[chars.len().saturating_sub(60)..].iter().collect()
    };
    assert!(
        tail.contains("z apple"),
        "tail of the dictation is missing — the open phrase was not sealed at stream end"
    );
    // The ratio bar has two sides and only one of them is lane-agnostic.
    //
    // BELOW the floor means lost spans — the capture dropped speech. That is a
    // regression in ANY lane, so the floor gates both.
    //
    // ABOVE the ceiling is the ambiguous side: it means either duplicated
    // phrases (a regression) or a layer that gap-filled toward what was
    // actually SAID (the layered lane doing its job). The ceiling counts tokens
    // against the Apple ruler, and the spoken truth carries MORE tokens than
    // Apple transcribes (171 vs 195 on this fixture, pinned by
    // `apple_reference_is_a_ruler_not_the_truth`), so it caps the capture below
    // what was said and fires hardest exactly when Layer 1 is most accurate —
    // measured live at 190 tokens, ratio 1.11, a hard panic.
    let ratio = our_tokens as f32 / target_tokens.max(1) as f32;
    assert!(
        ratio >= RATIO_BAR_MIN,
        "word count ratio {ratio:.2} below the floor {RATIO_BAR_MIN} \
         (ours {our_tokens} vs Apple reference {target_tokens}) — lost spans: the capture \
         dropped speech. A real regression in either lane; see the word diff above."
    );
    if apple_rulers_gate {
        assert!(
            ratio <= RATIO_BAR_MAX,
            "word count ratio {ratio:.2} above the ceiling {RATIO_BAR_MAX} \
             (ours {our_tokens} vs Apple reference {target_tokens}) — in the layer0 lane this \
             is duplicated phrases, because Layer 0 has nothing to gap-fill WITH. \
             Do NOT make a layer less accurate to satisfy this bar; restating it is an \
             operator decision (see \
             .vibecrafted/plans/w12-layered-live-closure/reports/default-flip-memo-layered.md)."
        );
    } else if ratio > RATIO_BAR_MAX {
        // A ceiling breach is excusable in the layered lane only when the
        // excuse is VISIBLE. Without the human reference there is no way to
        // tell gap-fill from duplication, so the Apple ceiling gates after all
        // rather than waving a red arm through on a story.
        let acc = accuracy.as_ref().unwrap_or_else(|| {
            panic!(
                "word count ratio {ratio:.2} above the ceiling {RATIO_BAR_MAX} in the layer1 lane \
                 (ours {our_tokens} vs Apple reference {target_tokens}), and there is NO human \
                 transcription beside {} to tell gap-fill from duplication. \
                 The ceiling gates until the excuse can be measured.",
                clip.display()
            )
        });
        eprintln!(
            "parity ratio-ceiling {ratio:.2} > {RATIO_BAR_MAX} NOT APPLIED — layer1 lane, \
             gap-fill toward the spoken {} tokens (accuracy-vs-human {:.3}); the ceiling counts \
             against the Apple ruler and is the wrong instrument here. Read the word diff above \
             if you suspect duplication rather than gap-fill.",
            acc.reference_tokens, acc.similarity
        );
    }

    if apple_rulers_gate {
        assert!(
            similarity >= PARITY_SIMILARITY_BAR,
            "our capture path must reproduce the system Apple live output \
             (similarity {similarity:.3} < bar {PARITY_SIMILARITY_BAR}) — see the word diff above; \
             the reference's own artefacts are part of the spec (data_assets/README.md). \
             SFSpeech is nondeterministic at word level and the Layer-0 sample straddles this \
             bar (n=10: 0.778 / 0.898 / 0.909 ×3 / 0.920 / 0.924 ×2 / 0.931 ×2), so re-run \
             before believing a single red — and restating the bar is an operator decision."
        );
    } else {
        eprintln!(
            "parity verdict: layer1 lane judged on structure only (head, tail, no lost spans, \
             lane match). Apple-fidelity numbers above are measurements, not a verdict — \
             see AGENTS.md 'Which ruler gates which lane'."
        );
    }
}
