//! Operator-take replay harness: feed any WAV through the production overlay
//! pipeline and print what the canvas actually did.
//!
//! This is the tool that cracked the 2026-08-12 repetition: the operator's
//! live take was replayed offline through the real Apple lane, reproducing the
//! full-sentence re-commits in the lab, and then re-run after each fix to
//! measure the repetition dropping — all without another live dictation.
//!
//! Ignored by default: it needs a recording, a live SFSpeech bridge, and
//! minutes of wall clock. Run it deliberately:
//!
//! ```bash
//! CODESCRIBE_REPLAY_WAV=/path/to/take.wav \
//! CODESCRIBE_STT_ENGINE=apple \
//! CODESCRIBE_APPLE_STT_BRIDGE=/Applications/Codescribe.app/Contents/MacOS/codescribe-stt-bridge \
//! CODESCRIBE_BRIDGE_DISCLAIM=1 \
//! cargo test --test replay_take -- --ignored --nocapture
//! ```
//!
//! Notes from the incident that built this:
//! - Without `CODESCRIBE_STT_ENGINE=apple` the session router can take the
//!   VAD/Whisper path and an Apple-lane defect will NOT reproduce — the first
//!   replay of the incident did exactly that and returned a clean transcript.
//! - Without `CODESCRIBE_APPLE_STT_BRIDGE` the worker spawns by bare name,
//!   fails, and the session mills the whole take against a dead engine before
//!   admitting it at stop time.
//! - Take audio survives in `/var/folders/**/codescribe_recording_<epoch_ms>.wav`
//!   (the audio spill); copy it out before the OS purges the directory.
//!
//! W13-0: when the replay emits `UtteranceFinal.segments`, this harness prints
//! a word-span histogram (duration / overlap / restart). That is the only
//! honest pl-PL Apple-span measurement — the in-repo fixtures are synthetic.

#[path = "support/w13_clock.rs"]
mod w13_clock;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "diagnostic harness: needs CODESCRIBE_REPLAY_WAV and a live SFSpeech bridge"]
async fn replay_operator_take() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .init();

    let wav = std::env::var("CODESCRIBE_REPLAY_WAV")
        .expect("set CODESCRIBE_REPLAY_WAV to the recording to replay");
    let settings = codescribe_core::config::UserSettings::load();
    let replay = codescribe::controller::production_replay::replay_overlay_recording(
        std::path::Path::new(&wav),
        Some("pl".to_string()),
        &settings,
        codescribe_core::asr_session::GatewaySessionAvailability::Unavailable,
        codescribe::controller::production_replay::ProductionReplayLane::AppleLexicon,
    )
    .await
    .expect("replay");

    eprintln!("=== FINALS ===");
    let mut word_spans: Vec<(f32, f32)> = Vec::new();
    for event in &replay.events {
        if let codescribe_core::pipeline::contracts::EngineEvent::UtteranceFinal {
            utterance_id,
            text,
            segments,
            ..
        } = event
        {
            // Counts and span bounds only — never echo transcript content.
            eprintln!(
                "[{utterance_id}] chars={} words={} segments={}",
                text.chars().count(),
                text.split_whitespace().count(),
                segments.len()
            );
            for seg in segments {
                word_spans.push((seg.start_ts, seg.end_ts));
            }
        }
    }
    let (hist, overlap, restarts) = w13_clock::histogram_apple_word_spans(&word_spans);
    eprintln!(
        "=== APPLE WORD SPANS === n={} overlap={} restarts={}",
        word_spans.len(),
        overlap,
        restarts
    );
    for bucket in hist {
        eprintln!("  {:>8} {}", bucket.label, bucket.count);
    }
    eprintln!(
        "=== LIVE TEXT ({} chars) ===",
        replay.live_text.chars().count()
    );
    eprintln!("{}", replay.live_text);
    eprintln!(
        "=== BOUNDARY === finals={} unique={} repeated={} overlap={}",
        replay.boundary_evidence.final_count,
        replay.boundary_evidence.unique_final_id_count,
        replay.boundary_evidence.repeated_final_id_count,
        replay.boundary_evidence.overlapping_final_window_count,
    );
}
