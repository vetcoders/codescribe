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
    use codescribe::presentation::transcript_bus::{CleanTranscriptEvent, transcript_bus_path};
    use std::io::{Read, Seek, SeekFrom, Write as _};

    let path = transcript_bus_path();
    let mut offset = std::fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let mut pending = Vec::<u8>::new();
    // The ledger document as this reader last printed it, and whose session it
    // belongs to. Both live across lines, not across sessions.
    let mut document = String::new();
    let mut document_session: Option<String> = None;
    let mut seal_reported = false;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        eprintln!("codescribe live: app transcript bus -> live draft stdout");
        eprintln!("bus={} start=end stop=Ctrl-C", path.display());
        eprintln!(
            "language_hint={} owner=Codescribe.app",
            language.as_deref().unwrap_or("auto")
        );

        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    eprintln!("codescribe live: stopped");
                    return Ok(());
                }
                () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }

            let mut file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let file_len = file.metadata()?.len();
            if file_len < offset {
                offset = 0;
                pending.clear();
            }
            file.seek(SeekFrom::Start(offset))?;
            let mut chunk = Vec::new();
            file.read_to_end(&mut chunk)?;
            offset = offset.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            pending.extend_from_slice(&chunk);

            while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=newline).collect();
                let line = &line[..line.len().saturating_sub(1)];
                if line.is_empty() {
                    continue;
                }
                // ONE FILE, TWO SCHEMAS. Deserializing every line straight into
                // `CleanTranscriptEvent` made this reader both deaf and loud:
                // app sessions have written their text only on
                // `codescribe.transcript-evidence.v1` since 2026-08-27, so the
                // clean lane carried nothing but lifecycle, and every evidence
                // row printed a parse error. Dispatch on the schema instead.
                let value: serde_json::Value = match serde_json::from_slice(line) {
                    Ok(value) => value,
                    Err(error) => {
                        eprintln!("codescribe live: unreadable bus line: {error}");
                        continue;
                    }
                };
                let schema = value
                    .get("schema")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();

                if schema == EVIDENCE_SCHEMA {
                    let Ok(event) = serde_json::from_value::<LiveEvidenceEvent>(value) else {
                        continue;
                    };
                    // A new session restarts the document; carrying the previous
                    // one over would report the whole next take as a revision.
                    if document_session.as_deref() != Some(event.session_id.as_str()) {
                        document_session = Some(event.session_id.clone());
                        document.clear();
                        seal_reported = false;
                    }
                    match document_change(&document, &event.rendered_text) {
                        LiveDocumentChange::Unchanged => {}
                        LiveDocumentChange::Appended(tail) => {
                            let stdout = std::io::stdout();
                            let mut out = stdout.lock();
                            writeln!(out, "{tail}")?;
                            out.flush()?;
                        }
                        LiveDocumentChange::Revised { from_char, tail } => {
                            // stdout is append-only, so a replacement cannot be
                            // unprinted. Say on stderr where it landed and print
                            // the corrected tail, rather than silently repeating
                            // the whole document.
                            eprintln!(
                                "codescribe live: revision replaced from char {from_char} ({} chars) session={}",
                                document.chars().count().saturating_sub(from_char),
                                &event.session_id[..8.min(event.session_id.len())],
                            );
                            let stdout = std::io::stdout();
                            let mut out = stdout.lock();
                            writeln!(out, "{tail}")?;
                            out.flush()?;
                        }
                    }
                    document = event.rendered_text;
                    // A terminal seal emits one row per document entry — eight
                    // on a real take — all carrying the same finished text. The
                    // seal is one event to a reader, so announce it once.
                    if event.reducer_action == "record_ledger_terminal_seal" {
                        if !seal_reported {
                            seal_reported = true;
                            eprintln!(
                                "codescribe live: terminal seal session={} chars={}",
                                &event.session_id[..8.min(event.session_id.len())],
                                document.chars().count()
                            );
                        }
                    } else {
                        seal_reported = false;
                    }
                    continue;
                }

                let Ok(event) = serde_json::from_value::<CleanTranscriptEvent>(value) else {
                    continue;
                };
                if let Some(text) = live_event_text(&event.status, &event.text) {
                    let stdout = std::io::stdout();
                    let mut out = stdout.lock();
                    writeln!(out, "{text}")?;
                    out.flush()?;
                } else if event.status == "utterance_revised" {
                    eprintln!(
                        "codescribe live: revision available session={} utterance={}",
                        event.session_id,
                        event
                            .utterance_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    );
                } else if event.status == "transcript_sealed" {
                    eprintln!(
                        "codescribe live: transcript sealed session={} chars={}",
                        event.session_id,
                        event.text.chars().count()
                    );
                }
            }
        }
    })
}

/// The committed-projection family the app actually writes its words to.
const EVIDENCE_SCHEMA: &str = "codescribe.transcript-evidence.v1";

/// The few evidence fields a live reader needs. Deliberately narrow: the full
/// event carries acoustic receipts and coverage that a text follower must not
/// depend on, and that would break this reader every time they change shape.
#[derive(serde::Deserialize)]
struct LiveEvidenceEvent {
    session_id: String,
    reducer_action: String,
    #[serde(default)]
    rendered_text: String,
}

/// How the ledger document moved, from the point of view of a reader that has
/// already printed `previous` and can never take it back.
#[derive(Debug, PartialEq, Eq)]
enum LiveDocumentChange<'a> {
    Unchanged,
    /// The document only grew: print the new tail.
    Appended(&'a str),
    /// The document was rewritten from `from_char` onward. `tail` is the
    /// corrected remainder; the characters before it still stand.
    Revised {
        from_char: usize,
        tail: &'a str,
    },
}

/// Classify a ledger document transition for an append-only stream.
///
/// The reducer both appends and replaces (measured on a real take: three
/// replacements among eight revisions), so a reader that assumes growth prints
/// the whole document again on every correction. Splitting on the first
/// differing character keeps stdout to the words themselves.
fn document_change<'a>(previous: &str, current: &'a str) -> LiveDocumentChange<'a> {
    if current == previous {
        return LiveDocumentChange::Unchanged;
    }
    // Char-wise, not byte-wise: a Polish diacritic is multi-byte and slicing a
    // byte offset would panic.
    let mut shared_bytes = 0usize;
    let mut shared_chars = 0usize;
    for (left, right) in previous.chars().zip(current.chars()) {
        if left != right {
            break;
        }
        shared_bytes += left.len_utf8();
        shared_chars += 1;
    }
    let tail = current[shared_bytes..].trim();
    if shared_chars == previous.chars().count() {
        if tail.is_empty() {
            return LiveDocumentChange::Unchanged;
        }
        return LiveDocumentChange::Appended(tail);
    }
    LiveDocumentChange::Revised {
        from_char: shared_chars,
        tail,
    }
}

/// Plain stdout is intentionally append-only and therefore shows each new draft
/// slot once. Revisions and the final seal remain machine-readable in the
/// canonical NDJSON bus and are announced on stderr without transcript content.
fn live_event_text<'a>(status: &str, text: &'a str) -> Option<&'a str> {
    if status != "utterance_draft" {
        return None;
    }
    let text = text.trim();
    (!text.is_empty()).then_some(text)
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
    if schema == EVIDENCE_SCHEMA {
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

    if let Some(lane) = lane.as_mut()
        && let Err(error) = lane.publish_sealed(&transcript_text, &verdict.raw.segments)
    {
        eprintln!("bus seal write failed: {error}");
    }

    // Delivery on stdout (the stream view already printed the canvas; the
    // delivery still follows it so scripts always end with the final text).
    if stream {
        eprintln!("--- delivery ---");
    }
    println!("{transcript_text}");

    // Provenance to stderr, GUI-truth style.
    eprintln!(
        "engine={:?}/{:?} decode_secs={:.2} segments={} chars={} avg_logprob={} transcript_authority=stt_verdict",
        verdict.engine.engine,
        verdict.engine.mode,
        decode_secs,
        verdict.raw.segments.len(),
        transcript_text.chars().count(),
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

    /// The witness is what an append-only stream would PRINT, replayed over a
    /// real revision trace (session `2bf9343a`: appends at revisions 1/5/12/18,
    /// replacements at 23/25/27, append at 29). A reader that treats every
    /// revision as growth reprints the whole document each time; joining what
    /// this classifier emits must reproduce the final document exactly once.
    #[test]
    fn replaying_a_ledger_never_prints_the_document_twice() {
        let revisions = [
            "alfa",
            "alfa beta",
            "alfa beta gamma",
            "alfa beta gama",       // replacement: the reducer rewrote the tail
            "alfa beta gama delta", // append onto the replacement
        ];
        let mut document = String::new();
        let mut printed: Vec<String> = Vec::new();
        let mut revisions_seen = 0;
        for revision in revisions {
            match document_change(&document, revision) {
                LiveDocumentChange::Unchanged => {}
                LiveDocumentChange::Appended(tail) => printed.push(tail.to_string()),
                LiveDocumentChange::Revised { tail, .. } => {
                    revisions_seen += 1;
                    printed.push(tail.to_string());
                }
            }
            document = revision.to_string();
        }

        assert_eq!(revisions_seen, 1, "the replacement must be reported as one");
        // Every printed piece is short: nothing reprinted the whole document.
        assert!(
            printed.iter().all(|piece| piece.len() < document.len()),
            "a piece as long as the document means the reader reprinted it: {printed:?}"
        );
        // And the final line carries the correction, not the superseded text.
        assert_eq!(printed.last().map(String::as_str), Some("delta"));
    }

    #[test]
    fn a_document_that_only_grows_prints_only_its_new_tail() {
        assert_eq!(
            document_change("alfa beta", "alfa beta gamma"),
            LiveDocumentChange::Appended("gamma")
        );
        assert_eq!(
            document_change("alfa", "alfa"),
            LiveDocumentChange::Unchanged
        );
        assert_eq!(
            document_change("alfa", "alfa   "),
            LiveDocumentChange::Unchanged
        );
    }

    /// Polish text is multi-byte. The shared prefix here spans 8 characters but
    /// 12 bytes, so an implementation that slices at the character count lands
    /// mid-codepoint and panics — that gap is the whole point of the test.
    #[test]
    fn a_revision_inside_a_diacritic_splits_on_a_character_not_a_byte() {
        let change = document_change("zażółć gesla jazn", "zażółć gęślą jaźń");
        let LiveDocumentChange::Revised { from_char, tail } = change else {
            panic!("expected a revision, got {change:?}");
        };
        assert_eq!(from_char, 8, "diverges at the ninth character");
        assert_ne!(
            from_char,
            "zażółć g".len(),
            "byte and char offsets must differ"
        );
        assert_eq!(tail, "ęślą jaźń");
    }

    #[test]
    fn live_plain_text_emits_only_nonempty_new_drafts() {
        assert_eq!(
            live_event_text("utterance_draft", "  instrukcja  "),
            Some("instrukcja")
        );
        assert_eq!(live_event_text("utterance_draft", "  "), None);
        assert_eq!(live_event_text("utterance_revised", "poprawka"), None);
        assert_eq!(live_event_text("transcript_sealed", "całość"), None);
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
