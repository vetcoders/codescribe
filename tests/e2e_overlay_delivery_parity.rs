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
use codescribe_core::pipeline::streaming::{
    assemble_live_from_events, collect_buffered_engine_events,
};
use codescribe_core::quality::{MergeMode, merge_live_whisper};
use codescribe_core::stt;

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{STT_OPT_IN_ENV, skip_unless_opt_in};

// ── Product live assembly (shipped `assemble_live_from_events`) ───────────────

fn overlay_assembly_from_events(events: &[EngineEvent]) -> String {
    assemble_live_from_events(events).full_text()
}

fn streaming_floor_from_events(events: &[EngineEvent]) -> String {
    assemble_live_from_events(events).streaming_floor()
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
    let assembly = assemble_live_from_events(events);
    let sealed = assembly.sealed_count();
    let full = assembly.full_text();
    let chars = full.chars().count();

    eprintln!(
        "  engine bar: sealed_finals={sealed} freezed={:?} full_chars={chars}",
        assembly
            .freezed
            .iter()
            .map(|s| s.chars().count())
            .collect::<Vec<_>>()
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
    let assembly = assemble_live_from_events(&events);
    assert_eq!(assembly.sealed_count(), 1);
    assert!(assembly.full_text().chars().count() < 40);
    // Engine bar would fail: sealed < 2 and chars < 120.
    assert!(
        assembly.sealed_count() < 2 || assembly.full_text().chars().count() < 120,
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

    let assembly = assemble_live_from_events(&events);
    assert_eq!(assembly.sealed_count(), 1);
    assert_eq!(assembly.freezed, vec!["pierwsze zdanie".to_string()]);
    assert_eq!(assembly.preview, "drugie zdanie live");
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

    let live = assemble_live_from_events(&events);
    let overlay = live.full_text();
    let stream_floor = live.streaming_floor();
    let preview_count = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::Preview { .. }))
        .count();
    let final_count = live.sealed_count();

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
    let live = assemble_live_from_events(&events);
    let transcript = {
        let full = live.full_text();
        if full.trim().is_empty() {
            live.streaming_floor()
        } else {
            full
        }
    };
    eprintln!(
        "transcript chars={} sealed={} events={}",
        transcript.chars().count(),
        live.sealed_count(),
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

#[tokio::test(flavor = "multi_thread")]
async fn e2e_device_capture_dictation() {
    if std::env::var("CODESCRIBE_E2E_CAPTURE_VIA_DEVICE")
        .ok()
        .as_deref()
        != Some("1")
    {
        eprintln!(
            "skip: set CODESCRIBE_E2E_CAPTURE_VIA_DEVICE=1 (run via scripts/e2e-blackhole-dictation.sh)"
        );
        return;
    }
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

#[tokio::test(flavor = "multi_thread")]
async fn e2e_apple_live_parity() {
    if std::env::var("CODESCRIBE_E2E_CAPTURE_VIA_DEVICE")
        .ok()
        .as_deref()
        != Some("1")
    {
        eprintln!("skip: run via `make test-engine-parity` (needs the BlackHole loopback)");
        return;
    }
    let device = std::env::var("AUDIO_INPUT_DEVICE")
        .expect("parity run requires AUDIO_INPUT_DEVICE (the loopback device name)");

    let clip = std::env::var("CODESCRIBE_E2E_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets/data_assets/05_apple-live-parity.wav")
        });
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

    let (_events, transcript, clip_seconds) = capture_clip_via_device(&device, &clip).await;
    assert!(
        !transcript.trim().is_empty(),
        "capture path produced NO text from {clip_seconds:.1}s of speech"
    );

    let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let ours = normalize(&transcript);
    let target = normalize(&reference);

    // Word-level diff via the Teacher aligner — the same instrument the
    // product uses for live↔whisper diffs, here diffing target↔ours.
    use codescribe_core::quality::teacher::{AlignOp, align_words, tokenize};
    let target_tokens = tokenize(&target);
    let our_tokens = tokenize(&ours);
    let ops = align_words(&target_tokens, &our_tokens);
    let equal = ops
        .iter()
        .filter(|op| matches!(op, AlignOp::Equal { .. }))
        .count();
    let similarity = equal as f32 / target_tokens.len().max(our_tokens.len()).max(1) as f32;
    eprintln!(
        "parity similarity {equal}/{} = {similarity:.3} (bar: exact match)",
        target_tokens.len().max(our_tokens.len())
    );
    {
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

    assert_eq!(
        ours, target,
        "our capture path must reproduce the system Apple live output \
         (similarity {similarity:.3}) — see the word diff above; the reference's \
         own artefacts are part of the spec (data_assets/README.md)"
    );
}
