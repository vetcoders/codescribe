//! codescribe CLI — file transcription that speaks the SAME pipeline as the GUI.
//!
//! WHY THIS EXISTS AGAIN. The previously installed `codescribe` binary was
//! v0.11.0 (built 2026-06-08) and the crate then dropped its default bin
//! entirely, so `make install` silently stopped replacing it — every
//! `codescribe transcribe` since ran a pre-layered pipeline two months behind
//! the product (operator, 2026-08-09: "dostosuj tryby transcribe do tego czym
//! rzeczywiście gada codescribe na GUI"). This bin routes through the exact
//! stages a GUI delivery does, in the same order:
//!
//!   Whisper file final → canonical transcript verdict
//!   `transcribe_file_verdict`
//!
//! Two faces, matching the GUI's two faces:
//! - default        = the DELIVERY: one shaped transcript on stdout.
//! - `--stream` = the LIVE CANVAS view: per-segment text flushed to stdout
//!   as decoding progresses through the file.
//! - `transcribe live` = follow the app-owned clean transcript bus and flush
//!   newly created utterance drafts to stdout one line at a time. Revisions and
//!   the final product seal remain explicit bus events. It never opens a
//!   second microphone or reconstructs text from UI previews.
//!
//! Provenance goes to stderr, GUI-truth style, so stdout stays pipeable.
//! The old `daemon` mode is gone on purpose: the SwiftUI app owns runtime.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "codescribe",
    version,
    about = "Local speech-to-text — the same pipeline the codescribe app delivers"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Transcribe a file or follow the app-owned live transcript bus
    Transcribe {
        /// Path to the audio file (omit when using `transcribe live`)
        file: Option<std::path::PathBuf>,
        /// File language; live accepts it for compatibility but app settings own capture
        #[arg(short, long, global = true)]
        language: Option<String>,
        /// Live-canvas view: flush each decoded segment as it lands
        #[arg(long)]
        stream: bool,
        /// Print only; do not publish this verdict onto the transcript bus
        #[arg(long)]
        no_bus: bool,
        #[command(subcommand)]
        mode: Option<TranscribeMode>,
    },
}

#[derive(Subcommand)]
enum TranscribeMode {
    /// Follow the app's transcript draft/seal bus; Ctrl-C closes the reader
    Live,
    /// Print the last completed transcript from the bus; stdout carries the
    /// words and nothing else, so a shell widget can insert it verbatim
    Last,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Transcribe {
            file,
            language,
            stream,
            no_bus,
            mode,
        } => match mode {
            Some(TranscribeMode::Live) => {
                anyhow::ensure!(
                    file.is_none() && !stream,
                    "`transcribe live` does not accept a file or --stream"
                );
                transcribe_live(language)
            }
            Some(TranscribeMode::Last) => {
                anyhow::ensure!(
                    file.is_none() && !stream,
                    "`transcribe last` does not accept a file or --stream"
                );
                transcribe_last()
            }
            None => {
                let file = file.ok_or_else(|| {
                    anyhow::anyhow!("missing <FILE> (or use `codescribe transcribe live`)")
                })?;
                transcribe(&file, language.as_deref(), stream, !no_bus)
            }
        },
    }
}

fn transcribe_live(language: Option<String>) -> anyhow::Result<()> {
    use codescribe::presentation::transcript_bus::transcript_bus_path;
    use codescribe::presentation::transcript_projection::{
        TranscriptBusFileWake, TranscriptProjectionReader,
    };
    use std::io::{Read, Seek, SeekFrom, Write as _};
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt as _;

    let path = transcript_bus_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut offset = std::fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    #[cfg(unix)]
    let mut file_identity = std::fs::metadata(&path)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()));
    let mut reader = TranscriptProjectionReader::new();
    let mut wake = TranscriptBusFileWake::new(&path)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        eprintln!("codescribe live: app transcript bus -> full projection JSONL stdout");
        eprintln!("bus={} start=end stop=Ctrl-C", path.display());
        eprintln!(
            "language_hint={} owner=Codescribe.app",
            language.as_deref().unwrap_or("auto")
        );

        loop {
            let wait = tokio::task::spawn_blocking(move || {
                let result = wake.wait(std::time::Duration::from_secs(2));
                (wake, result)
            });
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    eprintln!("codescribe live: stopped");
                    return Ok(());
                }
                result = wait => {
                    let (returned_wake, wait_result) = result?;
                    wake = returned_wake;
                    wait_result?;
                }
            }

            let mut file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let metadata = file.metadata()?;
            let file_len = metadata.len();
            #[cfg(unix)]
            let identity_changed =
                file_identity.is_some_and(|identity| identity != (metadata.dev(), metadata.ino()));
            #[cfg(not(unix))]
            let identity_changed = false;
            if identity_changed || file_len < offset {
                offset = 0;
                reader.reset_authority();
                eprintln!("codescribe live: Bus rotation/truncation opened a new authority domain");
            }
            #[cfg(unix)]
            {
                file_identity = Some((metadata.dev(), metadata.ino()));
            }
            file.seek(SeekFrom::Start(offset))?;
            let mut chunk = Vec::new();
            file.read_to_end(&mut chunk)?;
            offset = offset.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            if !chunk.is_empty() {
                let (lines, errors) = live_projection_lines(&mut reader, &chunk)?;
                let stdout = std::io::stdout();
                let mut out = stdout.lock();
                for line in lines {
                    writeln!(out, "{line}")?;
                }
                for error in errors {
                    eprintln!("codescribe live: unreadable bus line: {error}");
                }
                out.flush()?;
            }
        }
    })
}

fn live_projection_lines(
    reader: &mut codescribe::presentation::transcript_projection::TranscriptProjectionReader,
    bytes: &[u8],
) -> Result<(Vec<String>, Vec<String>), serde_json::Error> {
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    for result in reader.push_bytes(bytes) {
        match result {
            Ok(projection) => lines.push(projection.normalized_json()?),
            Err(error) => errors.push(error.to_string()),
        }
    }
    Ok((lines, errors))
}

/// Read the bus once and hand the last completed transcript to stdout.
///
/// This is the CLI half of "paste straight into the terminal". It does not
/// paste: a synthetic Cmd+V would target the frontmost app, which is the very
/// terminal this process is holding — the delivery throne already refuses that
/// case as `refuse_paste_into_self`. Emitting the text lets the shell's own
/// line editor insert it under a key the operator presses, with no Accessibility
/// grant and no synthetic event in the trust path.
fn transcribe_last() -> anyhow::Result<()> {
    use anyhow::Context as _;
    use codescribe::presentation::transcript_bus::transcript_bus_path;
    use std::io::Write as _;

    let path = transcript_bus_path();
    let ndjson = std::fs::read_to_string(&path)
        .with_context(|| format!("no transcript bus at {}", path.display()))?;
    let tail = bus_tail(&ndjson).ok_or_else(|| {
        anyhow::anyhow!(
            "transcript bus at {} holds no completed transcript yet",
            path.display()
        )
    })?;

    // No trailing newline. Pasted into a shell prompt a newline is Enter, and
    // this text is meant to land in a command line the operator still edits.
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    write!(out, "{}", tail.text)?;
    out.flush()?;

    eprintln!(
        "codescribe last: session={} chars={} bus={}",
        &tail.session_id[..8.min(tail.session_id.len())],
        tail.text.chars().count(),
        path.display()
    );
    Ok(())
}

/// The transcript `transcribe last` hands over, and whose session it came from.
#[derive(Debug, PartialEq, Eq)]
struct BusTail {
    session_id: String,
    text: String,
}

/// The transcript text one bus line contributes, if it carries any.
///
/// This allowlist is the whole guard. Lifecycle rows carry an EMPTY `text`
/// field rather than omitting it, so emptiness — not absence — disqualifies
/// them; and an unrecognised status is refused outright, so a future receipt
/// row that happens to carry prose can never become what the operator pastes.
fn tail_text(value: &serde_json::Value) -> Option<&str> {
    let schema = value
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if schema == codescribe::presentation::transcript_projection::EVIDENCE_SCHEMA {
        let text = value
            .get("rendered_text")
            .and_then(serde_json::Value::as_str)?
            .trim();
        return (!text.is_empty()).then_some(text);
    }
    let text = value
        .get("text")
        .and_then(serde_json::Value::as_str)?
        .trim();
    if text.is_empty() {
        return None;
    }
    match value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
    {
        "transcript_sealed" | "utterance_draft" | "utterance_revised" => Some(text),
        // session_started / session_ended and anything unrecognised: a reader
        // that accepted every status would paste lifecycle noise.
        _ => None,
    }
}

/// Resolve the bus tail: the last row that carried transcript text, and the
/// session it belonged to.
///
/// Both schemas restate the entire document on every later row — evidence rows
/// in `rendered_text`, the clean lane in its seal — so the newest such row is
/// also the completest, and nothing is accumulated here. The reducer owns that.
///
/// Deliberately NOT deduplicating. Real dictation repeats itself: deliberate
/// rhyme, and sentences restarted mid-word. A client-side dedup would eat
/// spoken content to paper over a ledger defect that belongs to the reducer.
fn bus_tail(ndjson: &str) -> Option<BusTail> {
    let mut tail: Option<BusTail> = None;
    for line in ndjson.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(session_id) = value.get("session_id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(text) = tail_text(&value) else {
            continue;
        };
        tail = Some(BusTail {
            session_id: session_id.to_string(),
            text: text.to_string(),
        });
    }
    tail
}

fn transcribe(
    file: &std::path::Path,
    language: Option<&str>,
    stream: bool,
    publish_bus: bool,
) -> anyhow::Result<()> {
    use codescribe::presentation::cli_transcript_lane::CliTranscriptLane;
    use codescribe::presentation::transcript_bus::{TranscriptMode, TranscriptSessionEndReason};
    use std::io::Write as _;

    anyhow::ensure!(file.exists(), "file not found: {}", file.display());

    // The bus is an observer, never a gate: a transcription that cannot be
    // published must still print. Every failure below is reported on stderr and
    // then dropped.
    let mut lane = if publish_bus {
        CliTranscriptLane::open(uuid::Uuid::new_v4().to_string(), TranscriptMode::Dictation)
    } else {
        None
    };
    if publish_bus && lane.is_none() {
        eprintln!("bus=unavailable (transcription continues; nothing published)");
    }
    if let Some(lane) = lane.as_mut() {
        match lane.retain_source_wav(file) {
            Ok(wav) => eprintln!("wav={}", wav.display()),
            Err(error) => eprintln!("session wav retain failed: {error}"),
        }
    }

    let started = std::time::Instant::now();
    // The one legal file route — identical to the GUI's stop-path final pass.
    let verdict = codescribe_core::stt::transcribe_file_verdict(file, language)?;
    let decode_secs = started.elapsed().as_secs_f64();

    let stdout = std::io::stdout();

    let transcript_text = if stream && !verdict.raw.segments.is_empty() {
        // One segment = one utterance draft, the app's own grain; the lane owns
        // that loop so stdout and the bus cannot disagree about what a line is.
        let assembled = match lane.as_mut() {
            Some(lane) => lane
                .publish_segments(&verdict.raw.segments)
                .unwrap_or_else(|error| {
                    eprintln!("bus draft write failed: {error}");
                    CliTranscriptLane::segment_texts(&verdict.raw.segments)
                }),
            None => CliTranscriptLane::segment_texts(&verdict.raw.segments),
        };
        let mut out = stdout.lock();
        for line in &assembled {
            writeln!(out, "{line}")?;
        }
        out.flush()?;
        assembled.join(" ")
    } else {
        verdict.text.clone()
    };
    // L2: previews/drafts stay raw; seal and delivery take the custom lexicon.
    let delivered_text =
        codescribe_core::quality::overlay_quality::apply_custom_lexicon(&transcript_text);

    if let Some(lane) = lane.as_mut()
        && let Err(error) = lane.publish_sealed(&delivered_text, &verdict.raw.segments)
    {
        eprintln!("bus seal write failed: {error}");
    }

    // Delivery on stdout (the stream view already printed the canvas; the
    // delivery still follows it so scripts always end with the final text).
    if stream {
        eprintln!("--- delivery ---");
    }
    println!("{delivered_text}");

    // Provenance to stderr, GUI-truth style.
    eprintln!(
        "engine={:?}/{:?} decode_secs={:.2} segments={} chars={} avg_logprob={} transcript_authority=stt_verdict",
        verdict.engine.engine,
        verdict.engine.mode,
        decode_secs,
        verdict.raw.segments.len(),
        delivered_text.chars().count(),
        verdict
            .raw
            .avg_logprob
            .map(|v| std::format!("{v:.2}"))
            .unwrap_or_else(|| "n/a".into()),
    );

    if let Some(lane) = lane.as_mut() {
        if let Err(error) = lane.publish_ended(TranscriptSessionEndReason::Completed) {
            eprintln!("bus end write failed: {error}");
        }
        eprintln!(
            "bus={} session={} source=cli_file_verdict",
            lane.path().display(),
            lane.session_id()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_command_is_a_subcommand_not_a_file_named_live() {
        let cli = Cli::try_parse_from(["codescribe", "transcribe", "live", "--language", "pl"])
            .expect("live command should parse");
        let Command::Transcribe {
            file,
            language,
            mode,
            ..
        } = cli.command;
        assert!(file.is_none());
        assert_eq!(language.as_deref(), Some("pl"));
        assert!(matches!(mode, Some(TranscribeMode::Live)));
    }

    #[test]
    fn live_consumer_stdout_is_exact_full_snapshot_jsonl() {
        use codescribe::presentation::transcript_projection::TranscriptProjectionReader;

        let input = serde_json::json!({
            "schema": "codescribe.transcript-evidence.v1",
            "sequence": 9,
            "session_id": "session-a",
            "reducer_revision": 4,
            "reducer_action": "apply_ledger_decision",
            "occurrence_session_id": "session-a",
            "capture_epoch": 2,
            "sample_start": 100,
            "sample_end": 200,
            "document_index": 1,
            "rendered_text": "całkowicie przepisany dokument"
        })
        .to_string()
            + "\n";
        let mut reader = TranscriptProjectionReader::new();
        let (lines, errors) =
            live_projection_lines(&mut reader, input.as_bytes()).expect("projection serialization");
        assert!(errors.is_empty());
        assert_eq!(lines.len(), 1);
        let output: serde_json::Value =
            serde_json::from_str(&lines[0]).expect("normalized projection JSON");
        assert_eq!(output["kind"], "live_revision");
        assert_eq!(output["reducer_revision"], 4);
        assert_eq!(output["rendered_text"], "całkowicie przepisany dokument");
    }

    /// One evidence row as the app writes it. `session_ended` really does carry
    /// an empty `text` field on this bus, which is why emptiness disqualifies.
    fn evidence(session: &str, action: &str, rendered: &str) -> String {
        format!(
            r#"{{"schema":"codescribe.transcript-evidence.v1","session_id":"{session}","reducer_action":"{action}","rendered_text":"{rendered}"}}"#
        )
    }

    fn clean(session: &str, status: &str, text: &str) -> String {
        format!(
            r#"{{"schema":"codescribe.transcript.v1","session_id":"{session}","status":"{status}","text":"{text}"}}"#
        )
    }

    /// The witness is the TEXT handed to the shell, not which field it came
    /// from: an app take ends on seven identical terminal seals followed by a
    /// lifecycle row, and the shell must receive the transcript.
    #[test]
    fn an_app_take_hands_over_its_seal_and_never_the_lifecycle_row() {
        let bus = [
            clean("aaa", "session_started", ""),
            evidence("aaa", "apply_ledger_decision", "alfa"),
            evidence("aaa", "apply_ledger_decision", "alfa beta"),
            evidence("aaa", "record_ledger_terminal_seal", "alfa beta gamma"),
            evidence("aaa", "record_ledger_terminal_seal", "alfa beta gamma"),
            clean("aaa", "session_ended", ""),
        ]
        .join("\n");

        let tail = bus_tail(&bus).expect("an app take has a tail");
        assert_eq!(tail.text, "alfa beta gamma");
        assert_eq!(tail.session_id, "aaa");
    }

    /// The rows AFTER the seal are the trap. `session_ended` carries an empty
    /// `text`, and a status this reader does not know may carry prose; taking
    /// the last row, or the last row with a `text` key, hands over either an
    /// empty insert or a receipt instead of the transcript.
    #[test]
    fn rows_after_the_seal_never_displace_the_transcript() {
        let bus = [
            clean("bbb", "session_started", ""),
            clean("bbb", "utterance_draft", "alfa"),
            clean("bbb", "transcript_sealed", "alfa beta"),
            clean("bbb", "delivery_receipt", "wklejono do vc-terminal"),
            clean("bbb", "session_ended", ""),
        ]
        .join("\n");

        assert_eq!(bus_tail(&bus).expect("sealed session").text, "alfa beta");
    }

    /// Still speaking: no whole state exists, so the newest utterance is the
    /// honest answer rather than nothing at all.
    #[test]
    fn a_session_still_speaking_hands_over_its_newest_utterance() {
        let bus = [
            clean("ccc", "session_started", ""),
            clean("ccc", "utterance_draft", "alfa"),
            clean("ccc", "utterance_draft", "beta"),
        ]
        .join("\n");

        assert_eq!(bus_tail(&bus).expect("open session").text, "beta");
    }

    #[test]
    fn the_newest_session_supersedes_the_one_before_it() {
        let bus = [
            evidence("aaa", "record_ledger_terminal_seal", "stara wypowiedz"),
            clean("aaa", "session_ended", ""),
            evidence("bbb", "record_ledger_terminal_seal", "nowa wypowiedz"),
        ]
        .join("\n");

        let tail = bus_tail(&bus).expect("second session");
        assert_eq!(tail.text, "nowa wypowiedz");
        assert_eq!(tail.session_id, "bbb");
    }

    /// Nothing to paste must be nothing, not an empty insert: a widget that
    /// received "" would silently do nothing while looking like it worked.
    #[test]
    fn a_bus_carrying_only_lifecycle_hands_over_nothing() {
        let bus = [
            clean("aaa", "session_started", ""),
            clean("aaa", "session_ended", ""),
            "{ this line is not json".to_string(),
        ]
        .join("\n");

        assert_eq!(bus_tail(&bus), None);
    }
}
