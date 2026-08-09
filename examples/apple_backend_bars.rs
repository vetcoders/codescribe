//! W4-A: measure ONE Apple STT backend lane against a frozen reference.
//!
//! The lane is chosen by the environment, not by an argument, because that is
//! how the product chooses it too — `CODESCRIBE_APPLE_DICTATION_TRANSCRIBER=1`
//! arms the DictationTranscriber lane, absent/0 leaves the shipped SFSpeech
//! default. Running the binary twice under two environments therefore compares
//! the real routing, not a test-only switch. Each run reports which backend the
//! bridge actually selected, so a mis-set lane is visible instead of silent.
//!
//! Bars mirror `tests/e2e_overlay_delivery_parity.rs`: token similarity via the
//! Teacher aligner, word-count ratio, head present, tail sealed.
//!
//! Usage (fixtures live outside the repo — resolve them with the shared helper):
//!   CLIP=$(./scripts/lib/data-assets.sh resolve 05_apple-live-parity.wav)
//!   cargo run --release --example apple_backend_bars -- "$CLIP"
//!   CODESCRIBE_APPLE_DICTATION_TRANSCRIBER=1 \
//!     cargo run --release --example apple_backend_bars -- "$CLIP"
//!
//! Reference selection (beside the clip, per tests/assets/data_assets/README.md):
//!   default            `<stem>_human_transcription.txt`   — what was said
//!   REFERENCE=apple    `<stem>_apple_live_reference.txt`  — system dictation
//!
//! Optional bars for the 05 parity fixture (skipped when unset):
//!   HEAD_MARKER="teraz zaczynam"  TAIL_MARKER="z apple"
//!
//! A frozen lane (the SYSTEM dictation output captured once, beside the clip)
//! cannot be re-run, but it still deserves the same instrument — otherwise the
//! A/B/C table compares scores produced by different code. `LANE_TEXT=<suffix>`
//! scores `<stem><suffix>` as if the lane had just produced it:
//!   LANE_TEXT=_apple_live_reference.txt … REFERENCE=human

use std::path::{Path, PathBuf};
use std::time::Instant;

use codescribe_core::quality::teacher::{AlignOp, align_words, tokenize};

/// Resolve the reference transcript sitting beside the clip, chosen by the
/// `REFERENCE` variable: `apple` scores against system dictation, anything
/// else against the human transcription.
fn reference_path(clip: &Path) -> PathBuf {
    let suffix = match std::env::var("REFERENCE").as_deref() {
        Ok("apple") => "_apple_live_reference.txt",
        _ => "_human_transcription.txt",
    };
    let stem = clip
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    clip.with_file_name(format!("{stem}{suffix}"))
}

/// Score every clip on the command line and print one table row each.
///
/// The requested lane is echoed before any measurement, and each row also
/// carries the backend the bridge actually selected — a mis-set lane then
/// shows up in the output instead of quietly producing numbers for the wrong
/// engine. Head and tail markers are optional and report `n/a` when unset,
/// so the same harness serves fixtures that have no such anchors.
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let clips: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    anyhow::ensure!(
        !clips.is_empty(),
        "usage: apple_backend_bars <clip.wav> [clip.wav ...] \
         (resolve with ./scripts/lib/data-assets.sh resolve <name>)"
    );

    let lane_armed = matches!(
        std::env::var("CODESCRIBE_APPLE_DICTATION_TRANSCRIBER")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    );
    println!(
        "# lane requested: {}",
        if lane_armed {
            "dictation_transcriber (armed)"
        } else {
            "shipped default (SFSpeech/ST)"
        }
    );
    println!(
        "clip | backend | wall_ms | chars | tokens | ref_tokens | similarity | ratio | head | tail"
    );

    for clip in &clips {
        anyhow::ensure!(clip.exists(), "fixture not found: {}", clip.display());
        let reference_file = reference_path(clip);
        anyhow::ensure!(
            reference_file.exists(),
            "reference not found beside the clip: {}",
            reference_file.display()
        );
        let reference = std::fs::read_to_string(&reference_file)?;

        let started = Instant::now();
        let (lane_label, text) = match std::env::var("LANE_TEXT") {
            Ok(suffix) if !suffix.is_empty() => {
                let stem = clip
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                let frozen = clip.with_file_name(format!("{stem}{suffix}"));
                anyhow::ensure!(
                    frozen.exists(),
                    "frozen lane not found: {}",
                    frozen.display()
                );
                ("frozen".to_string(), std::fs::read_to_string(&frozen)?)
            }
            _ => {
                let verdict =
                    codescribe_core::stt::apple_stt::transcribe_file_verdict(clip, Some("pl"))?;
                (verdict.engine.mode.to_string(), verdict.text)
            }
        };
        let wall_ms = started.elapsed().as_millis();

        let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let ours = normalize(&text);
        let target = normalize(&reference);
        let our_tokens = tokenize(&ours);
        let ref_tokens = tokenize(&target);
        let ops = align_words(&ref_tokens, &our_tokens);
        let equal = ops
            .iter()
            .filter(|op| matches!(op, AlignOp::Equal { .. }))
            .count();
        let denominator = ref_tokens.len().max(our_tokens.len()).max(1);
        let similarity = equal as f32 / denominator as f32;
        let ratio = our_tokens.len() as f32 / ref_tokens.len().max(1) as f32;

        let lower = ours.to_lowercase();
        let mark = |value: Option<String>, present: Option<bool>| match (value, present) {
            (None, _) => "n/a".to_string(),
            (Some(_), Some(true)) => "ok".to_string(),
            (Some(_), _) => "MISSING".to_string(),
        };
        let head_marker = std::env::var("HEAD_MARKER").ok().filter(|m| !m.is_empty());
        let head_ok = head_marker.as_ref().map(|m| {
            lower
                .chars()
                .take(120)
                .collect::<String>()
                .contains(&m.to_lowercase())
        });
        let tail_marker = std::env::var("TAIL_MARKER").ok().filter(|m| !m.is_empty());
        let tail_ok = tail_marker.as_ref().map(|m| {
            let chars: Vec<char> = lower.chars().collect();
            chars[chars.len().saturating_sub(60)..]
                .iter()
                .collect::<String>()
                .contains(&m.to_lowercase())
        });

        println!(
            "{} | {} | {} | {} | {} | {} | {:.3} | {:.2} | {} | {}",
            clip.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            lane_label,
            wall_ms,
            text.chars().count(),
            our_tokens.len(),
            ref_tokens.len(),
            similarity,
            ratio,
            mark(head_marker, head_ok),
            mark(tail_marker, tail_ok),
        );
    }

    Ok(())
}
