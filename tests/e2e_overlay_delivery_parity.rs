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
use codescribe_core::pipeline::contracts::{
    EngineEvent, FINAL_PASS_REGRESSION_MIN_STREAM_CHARS, final_pass_is_length_regression,
};
use codescribe_core::pipeline::streaming::collect_buffered_engine_events;
use codescribe_core::stt;

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{STT_OPT_IN_ENV, skip_unless_opt_in};

// ── Overlay-shaped assembly (mirrors OverlayState.rawLiveText semantics) ─────

/// Freezed previous utterances + open interim preview (append relative to freezed).
///
/// This is the product live model: `applyPreview` only mutates the open tail;
/// `UtteranceFinal` freezes a segment and clears the tail.
fn overlay_assembly_from_events(events: &[EngineEvent]) -> String {
    let mut freezed: Vec<String> = Vec::new();
    let mut preview = String::new();

    for event in events {
        match event {
            EngineEvent::Preview { text, .. } => {
                // Utterance-local cumulative interim → replace open tail only.
                preview = text.trim().to_string();
            }
            EngineEvent::Correction { text, .. } => {
                preview = text.trim().to_string();
            }
            EngineEvent::UtteranceFinal { text, .. } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    freezed.push(trimmed.to_string());
                }
                preview.clear();
            }
            EngineEvent::NoSpeech { .. } => {
                preview.clear();
            }
            _ => {}
        }
    }

    let mut parts = freezed;
    let preview = preview.trim();
    if !preview.is_empty() {
        parts.push(preview.to_string());
    }
    parts.join(" ")
}

/// Stop-path streaming splice: sealed finals only (no open interim).
fn streaming_floor_from_events(events: &[EngineEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::UtteranceFinal { text, .. } => {
                let t = text.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Delivery after adjudicate length guard (stream is floor of truth).
fn delivery_from_stream_and_final(stream: &str, final_text: &str) -> (&'static str, String) {
    let stream = stream.trim();
    let final_text = final_text.trim();
    if final_text.is_empty() {
        return ("streaming_floor", stream.to_string());
    }
    if final_pass_is_length_regression(final_text, stream) {
        return ("streaming_floor_after_regression", stream.to_string());
    }
    ("final_pass", final_text.to_string())
}

fn data_assets_dir() -> PathBuf {
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
    assert!(
        !clips.is_empty(),
        "expected WAV fixtures under tests/assets/data_assets/"
    );
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

    // 1) Live path — identical to StreamingRecorder → transcription_session.
    let t0 = std::time::Instant::now();
    let events = collect_buffered_engine_events(&samples, sample_rate, language.clone())
        .await
        .unwrap_or_else(|e| panic!("live session on {}: {e}", clip.display()));
    eprintln!("  live session done in {:?}", t0.elapsed());

    let overlay = overlay_assembly_from_events(&events);
    let stream_floor = streaming_floor_from_events(&events);
    let preview_count = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::Preview { .. }))
        .count();
    let final_count = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::UtteranceFinal { .. }))
        .count();

    eprintln!(
        "  events: total={} previews={} utterance_finals={}",
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
        let preview: String = overlay.chars().take(120).collect();
        eprintln!("  overlay preview: {preview}…");
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
        "  final-pass text preview: {}…",
        final_verdict.text.chars().take(120).collect::<String>()
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

    if final_pass_is_length_regression(final_verdict.text.trim(), stream_for_floor) {
        assert_eq!(
            source, "streaming_floor_after_regression",
            "collapsing file final must not win over live assembly"
        );
        assert_eq!(delivery.trim(), stream_for_floor.trim());
        eprintln!("  ✓ length regression rejected (stream kept as floor)");
    } else {
        assert!(
            matches!(source, "final_pass" | "streaming_floor"),
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

    if let Some(human) = human_reference_for_wav(clip) {
        let human_lower = human.to_lowercase();
        let delivery_lower = delivery.to_lowercase();
        // Soft keyword overlap — not WER (engines differ); catch total garbage.
        let anchors = [
            "rust",
            "loctree",
            "codescribe",
            "kubernetes",
            "pacjent",
            "tokio",
        ];
        let human_hits: Vec<_> = anchors
            .iter()
            .filter(|a| human_lower.contains(**a))
            .collect();
        if !human_hits.is_empty() {
            let matched = human_hits
                .iter()
                .filter(|a| delivery_lower.contains(**a))
                .count();
            eprintln!(
                "  human-ref anchors in delivery: {matched}/{} ({human_hits:?})",
                human_hits.len()
            );
            // At least one shared anchor when human text mentions domain terms.
            assert!(
                matched >= 1,
                "delivery shares no anchors with human reference for {}\n\
                 delivery={delivery}\n\
                 human_hits={human_hits:?}",
                clip.display()
            );
        }
    }
}
