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
//! - `transcribe live` = follow the app-owned clean transcript bus. The default
//!   is the human canvas view: drafts append to the open line, a reducer
//!   rewrite is marked `⟲ rev N`, and a terminal seal closes the take as a
//!   permanent block. `--json` keeps the raw projection JSONL for machine
//!   consumers. It never opens a second microphone or reconstructs text from
//!   UI previews.
//! - multiple FILES transcribe sequentially; per-file headers go to stderr so
//!   stdout stays clean transcript text.
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
    /// Transcribe files or follow the app-owned live transcript bus
    ///
    /// stdout carries the payload and nothing else: transcript text in file
    /// mode, projection JSONL under --json. Headers, provenance and engine
    /// warnings go to stderr, so `... 2>/dev/null` needs no further filtering.
    Transcribe {
        /// Audio files to transcribe in order (omit when using `transcribe live`)
        files: Vec<std::path::PathBuf>,
        /// File language; live accepts it for compatibility but app settings own capture
        #[arg(short, long, global = true)]
        language: Option<String>,
        /// Print admitted segments after each decode window, without repeating the final text
        #[arg(long)]
        stream: bool,
        /// Print only; do not publish this verdict onto the transcript bus
        #[arg(long)]
        no_bus: bool,
        /// Live: raw projection JSONL for machine consumers instead of the human view
        #[arg(long, global = true)]
        json: bool,
        /// Literal words: skip the Light+ sentence shaping (the app's Ctrl-hold lane)
        #[arg(long)]
        raw: bool,
        /// Print the take truth under one time axis: segments, the 32 ms Silero
        /// row and the energy row (stderr, so stdout stays the transcript)
        #[arg(long)]
        inspect: bool,
        /// Do not write the <file>.truth.json observer sidecar beside the input
        #[arg(long)]
        no_truth: bool,
        #[command(subcommand)]
        mode: Option<TranscribeMode>,
    },
    /// Inspect and compact the clean transcript bus
    ///
    /// The bus carries two records with opposite retention needs: the delivery
    /// transcript, which is small and worth keeping, and the acoustic evidence,
    /// which is ~93% of the bytes and is consumed within days. `compact` drops
    /// aged evidence and keeps every delivery row.
    Bus {
        #[command(subcommand)]
        action: BusAction,
    },
    /// Inspect, replay and recover the custom pronunciation lexicon
    ///
    /// The lexicon is a PRE-LLM variant→canonical substitution table. It is
    /// grown two ways: by hand, and by replaying human corrections through the
    /// extractor. Only the second needs adjudicating, which is what `replay`
    /// reports and `--apply` obeys.
    Lexicon {
        #[command(subcommand)]
        action: LexiconAction,
    },
}

#[derive(Subcommand)]
enum BusAction {
    /// Size, composition and span of the bus
    Status,
    /// Drop evidence rows older than the retention window
    ///
    /// Refuses while a session may be open, and discards its own work rather
    /// than overwrite rows appended during the rewrite.
    Compact {
        /// Evidence retention in days
        #[arg(long, default_value_t = codescribe::presentation::transcript_bus_maintenance::DEFAULT_EVIDENCE_RETENTION_DAYS)]
        evidence_older_than: u32,
        /// Report what would be dropped without rewriting the bus
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum LexiconAction {
    /// Summarise the live lexicon: rows, variants, provenance, recoverable backups
    Show,
    /// Replay corrections through the extractor and the admission gate
    Replay {
        /// corrections.jsonl to replay (default: <config>/quality/corrections.jsonl)
        #[arg(long)]
        corrections: Option<std::path::PathBuf>,
        /// Where the three tier files and the report land (default: <config>)
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Write the accepted tier into the live lexicon; the others never land
        #[arg(long)]
        apply: bool,
    },
    /// Merge a rotation backup back into the live lexicon
    ///
    /// Hand-curated rows return verbatim; rows the extractor wrote are
    /// re-adjudicated through the same gate `replay` uses. The merge is a
    /// union, so nothing the live file gained after the backup is lost.
    Restore {
        /// Backup to restore from (default: the richest recoverable one)
        #[arg(long)]
        from: Option<std::path::PathBuf>,
        /// Report what would change without writing the lexicon
        #[arg(long)]
        dry_run: bool,
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
    // Engine warnings (a refused long-file span, a degraded lane) are the
    // CLI's only way to say "this transcript is missing something"; they go
    // to stderr, so `transcribe last` stdout stays verbatim for the widget.
    codescribe::logging::init_logging_with_default_filter("warn");
    let cli = Cli::parse();
    match cli.command {
        Command::Transcribe {
            files,
            language,
            stream,
            no_bus,
            json,
            raw,
            inspect,
            no_truth,
            mode,
        } => match mode {
            Some(TranscribeMode::Live) => {
                anyhow::ensure!(
                    files.is_empty() && !stream && !raw && !inspect && !no_truth,
                    "`transcribe live` does not accept a file, --stream, --raw, --inspect or --no-truth (the app decides the lane)"
                );
                transcribe_live(language, json)
            }
            Some(TranscribeMode::Last) => {
                anyhow::ensure!(
                    files.is_empty() && !stream && !json && !raw && !inspect && !no_truth,
                    "`transcribe last` does not accept a file, --stream, --json, --raw, --inspect or --no-truth"
                );
                transcribe_last()
            }
            None => {
                anyhow::ensure!(
                    !json,
                    "--json belongs to `transcribe live`; file mode already prints plain text"
                );
                anyhow::ensure!(
                    !files.is_empty(),
                    "missing <FILES> (or use `codescribe transcribe live`)"
                );
                transcribe_batch(
                    &files,
                    language.as_deref(),
                    stream,
                    !no_bus,
                    raw,
                    inspect,
                    !no_truth,
                )
            }
        },
        Command::Bus { action } => run_bus(action),
        Command::Lexicon { action } => run_lexicon(action),
    }
}

/// Transcript bus surface: what is in it, and how to get the space back.
fn run_bus(action: BusAction) -> anyhow::Result<()> {
    use codescribe::presentation::transcript_bus_maintenance::{bus_status, compact_bus};

    let path = codescribe::presentation::transcript_bus::transcript_bus_path();
    match action {
        BusAction::Status => {
            let status = bus_status(&path)?;
            println!("bus: {}", status.path.display());
            println!("size: {}", human_bytes(status.bytes));
            println!(
                "delivery rows: {} ({})",
                status.delivery_rows,
                human_bytes(status.delivery_bytes)
            );
            println!(
                "evidence rows: {} ({})",
                status.evidence_rows,
                human_bytes(status.evidence_bytes)
            );
            if status.other_rows > 0 {
                println!("other rows: {} (never compacted away)", status.other_rows);
            }
            if let (Some(first), Some(last)) = (&status.first_seen, &status.last_seen) {
                println!("span: {first} .. {last}");
            }
            if !status.retention_preview.is_empty() {
                println!("reclaimable by evidence retention window:");
                for (days, bytes) in &status.retention_preview {
                    println!("  {days:>3}d  {}", human_bytes(*bytes));
                }
            }
            if status.wants_compaction() {
                println!(
                    "past the compaction threshold — pick a window and run \
                     `codescribe bus compact --evidence-older-than <days>` \
                     with Codescribe stopped"
                );
            }
            Ok(())
        }
        BusAction::Compact {
            evidence_older_than,
            dry_run,
        } => {
            let report = compact_bus(&path, evidence_older_than, dry_run)?;
            println!(
                "rows: {} read, {} kept, {} aged-out evidence dropped",
                report.rows_read, report.rows_kept, report.evidence_rows_dropped
            );
            println!(
                "size: {} -> {} ({} reclaimed){}",
                human_bytes(report.bytes_before),
                human_bytes(report.bytes_after),
                human_bytes(report.bytes_reclaimed()),
                if report.applied {
                    ""
                } else {
                    " — dry run, bus untouched"
                }
            );
            Ok(())
        }
    }
}

/// Byte count an operator can read at a glance.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Lexicon surface: summarise, replay, recover.
fn run_lexicon(action: LexiconAction) -> anyhow::Result<()> {
    use codescribe_core::config::Config;
    use codescribe_core::quality::lexicon_replay::run_lexicon_replay;
    use codescribe_core::quality::lexicon_restore::{
        newest_recoverable_backup, restore_custom_lexicon_from_backup,
    };

    let config_dir = Config::config_dir();
    match action {
        LexiconAction::Show => {
            let path = config_dir.join("lexicon.custom.jsonl");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let mut rows = 0usize;
            let mut variants = 0usize;
            let mut curated = 0usize;
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                rows += 1;
                variants += value
                    .get("mispronunciations")
                    .and_then(|v| v.as_array())
                    .map_or(0, |a| a.len());
                if value.get("source").and_then(|v| v.as_str()) != Some("correction") {
                    curated += 1;
                }
            }
            println!("lexicon: {}", path.display());
            println!("rows: {rows}  variants: {variants}");
            println!("curated: {curated}  from corrections: {}", rows - curated);
            match newest_recoverable_backup(&config_dir) {
                // A richer backup than the live file means the live file lost
                // rules. Saying so here is the whole point of `show`.
                Some(backup) => println!(
                    "recoverable backup holds MORE rows than the live file: {}\n\
                     run `codescribe lexicon restore` to merge it back",
                    backup.display()
                ),
                None => println!("no backup holds more than the live file"),
            }
            Ok(())
        }
        LexiconAction::Replay {
            corrections,
            out,
            apply,
        } => {
            let source =
                corrections.unwrap_or_else(|| config_dir.join("quality").join("corrections.jsonl"));
            let out_dir = out.unwrap_or_else(|| config_dir.clone());
            let outcome = run_lexicon_replay(&source, &out_dir, apply)?;
            eprintln!(
                "replay: {} candidate pair(s), {} accepted{}",
                outcome.table.len(),
                outcome.accepted(),
                if apply { " and applied" } else { " (dry-run)" }
            );
            for (tier, count) in &outcome.tier_counts {
                eprintln!("  {tier}: {count}");
            }
            eprintln!("accepted: {}", outcome.accepted_path.display());
            eprintln!("review:   {}", outcome.review_path.display());
            eprintln!("rejected: {}", outcome.rejected_path.display());
            eprintln!("report:   {}", outcome.report_path.display());
            Ok(())
        }
        LexiconAction::Restore { from, dry_run } => {
            let backup = match from {
                Some(path) => path,
                None => newest_recoverable_backup(&config_dir).ok_or_else(|| {
                    anyhow::anyhow!(
                        "no backup in {} holds more rows than the live lexicon",
                        config_dir.display()
                    )
                })?,
            };
            if dry_run {
                // Reading is free; the caller asked not to write, so stop before
                // the merge rather than writing and reporting after the fact.
                println!("would restore from {}", backup.display());
                println!("run without --dry-run to merge it into the live lexicon");
                return Ok(());
            }
            let report = restore_custom_lexicon_from_backup(&backup)?;
            println!("restored from {}", backup.display());
            println!(
                "backup rows: {}  curated restored: {}  auto examined: {}  auto pairs accepted: {}",
                report.backup_rows,
                report.curated_rows_restored,
                report.auto_rows_examined,
                report.auto_pairs_accepted
            );
            println!(
                "live rows: {} -> {}  variants now: {}",
                report.live_rows_before, report.live_rows_after, report.variants_after
            );
            Ok(())
        }
    }
}

/// Transcribe files in order. One failing file reports on stderr and the batch
/// continues; the exit code stays non-zero so scripts still see the failure.
fn transcribe_batch(
    files: &[std::path::PathBuf],
    language: Option<&str>,
    stream: bool,
    publish_bus: bool,
    raw: bool,
    inspect: bool,
    write_truth: bool,
) -> anyhow::Result<()> {
    let mut failures = Vec::new();
    for (index, file) in files.iter().enumerate() {
        if files.len() > 1 {
            eprintln!(
                "--- file {}/{}: {} ---",
                index + 1,
                files.len(),
                file.display()
            );
            if index > 0 {
                // Batch stdout stays parseable: one blank line between transcripts.
                println!();
            }
        }
        if let Err(error) = transcribe(
            file,
            language,
            stream,
            publish_bus,
            raw,
            inspect,
            write_truth,
        ) {
            eprintln!("FAILED {}: {error:#}", file.display());
            failures.push(file.display().to_string());
        }
    }
    anyhow::ensure!(
        failures.is_empty(),
        "{} of {} files failed: {}",
        failures.len(),
        files.len(),
        failures.join(", ")
    );
    Ok(())
}

/// What the human view already has on the open terminal line, so the next
/// projection can be rendered as a delta instead of a full reprint.
#[derive(Debug, Default)]
struct LiveHumanView {
    /// Session and full rendered text of the line currently left open.
    open_line: Option<(String, String)>,
}

impl LiveHumanView {
    /// Exact bytes to write for one projection. Appends leave the line open;
    /// a revision that is not a pure extension closes it and marks `⟲ rev N`;
    /// a terminal seal closes the take as a permanent block. No cursor moves —
    /// works identically in tmux, zellij, and a bare tty.
    fn render(
        &mut self,
        projection: &codescribe::presentation::transcript_projection::TranscriptProjection,
    ) -> String {
        use codescribe::presentation::transcript_projection::TranscriptProjectionKind;

        let session = projection.session_id.as_str();
        let text = projection.rendered_text.as_str();
        match projection.kind {
            TranscriptProjectionKind::LiveRevision => match self.open_line.take() {
                Some((open_session, previous)) if open_session == session => {
                    if let Some(appended) = text.strip_prefix(previous.as_str()) {
                        self.open_line = Some((open_session, text.to_string()));
                        appended.to_string()
                    } else {
                        self.open_line = Some((open_session, text.to_string()));
                        format!("\n⟲ rev {}: {text}", projection.reducer_revision)
                    }
                }
                interrupted => {
                    // None = fresh canvas; Some(other session) = close that line first.
                    let prefix = if interrupted.is_some() { "\n" } else { "" };
                    self.open_line = Some((session.to_string(), text.to_string()));
                    format!("{prefix}{text}")
                }
            },
            TranscriptProjectionKind::TerminalSeal => {
                let newline = if self.open_line.take().is_some() {
                    "\n"
                } else {
                    ""
                };
                format!(
                    "{newline}⏺ sealed · session {} · rev {} · samples {}..{}\n{text}\n\n",
                    &session[..8.min(session.len())],
                    projection.reducer_revision,
                    projection.sample_start,
                    projection.sample_end,
                )
            }
        }
    }
}

fn transcribe_live(language: Option<String>, json: bool) -> anyhow::Result<()> {
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

    let mut human_view = (!json).then(LiveHumanView::default);

    runtime.block_on(async move {
        if json {
            eprintln!("codescribe live: app transcript bus -> full projection JSONL stdout");
        } else {
            eprintln!(
                "codescribe live: canvas view (drafts append, ⟲ marks a rewrite, ⏺ seals a take); --json for raw projections"
            );
        }
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
                let (projections, errors) = live_projections(&mut reader, &chunk);
                let stdout = std::io::stdout();
                let mut out = stdout.lock();
                match human_view.as_mut() {
                    Some(view) => {
                        for projection in &projections {
                            write!(out, "{}", view.render(projection))?;
                        }
                    }
                    None => {
                        for projection in &projections {
                            writeln!(out, "{}", projection.normalized_json()?)?;
                        }
                    }
                }
                for error in errors {
                    eprintln!("codescribe live: unreadable bus line: {error}");
                }
                out.flush()?;
            }
        }
    })
}

fn live_projections(
    reader: &mut codescribe::presentation::transcript_projection::TranscriptProjectionReader,
    bytes: &[u8],
) -> (
    Vec<codescribe::presentation::transcript_projection::TranscriptProjection>,
    Vec<String>,
) {
    let mut projections = Vec::new();
    let mut errors = Vec::new();
    for result in reader.push_bytes(bytes) {
        match result {
            Ok(projection) => projections.push(projection),
            Err(error) => errors.push(error.to_string()),
        }
    }
    (projections, errors)
}

#[cfg(test)]
fn live_projection_lines(
    reader: &mut codescribe::presentation::transcript_projection::TranscriptProjectionReader,
    bytes: &[u8],
) -> Result<(Vec<String>, Vec<String>), serde_json::Error> {
    let (projections, errors) = live_projections(reader, bytes);
    let lines = projections
        .iter()
        .map(|projection| projection.normalized_json())
        .collect::<Result<Vec<_>, _>>()?;
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
    raw: bool,
    inspect: bool,
    write_truth: bool,
) -> anyhow::Result<()> {
    use codescribe::presentation::cli_transcript_lane::CliTranscriptLane;
    use codescribe::presentation::transcript_bus::{TranscriptMode, TranscriptSessionEndReason};
    use codescribe_core::pipeline::take_truth::{TakeTruth, write_truth_sidecar};
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
    let stdout = std::io::stdout();
    let mut streamed_text = String::new();
    let verdict =
        codescribe_core::stt::transcribe_file_verdict_observed(file, language, &mut |segments| {
            if stream {
                let lines = match lane.as_mut() {
                    Some(lane) => lane.publish_segments(segments).unwrap_or_else(|error| {
                        eprintln!("bus draft write failed: {error}");
                        CliTranscriptLane::segment_texts(segments)
                    }),
                    None => CliTranscriptLane::segment_texts(segments),
                };
                let mut out = stdout.lock();
                for line in lines {
                    writeln!(out, "{line}")?;
                    if !streamed_text.is_empty() {
                        streamed_text.push(' ');
                    }
                    streamed_text.push_str(&line);
                }
                out.flush()?;
            }
            Ok(())
        });
    let verdict = match verdict {
        Ok(verdict) => verdict,
        Err(error) => {
            if let Some(lane) = lane.as_mut()
                && let Err(bus_error) =
                    lane.publish_ended(TranscriptSessionEndReason::TranscriptionFailed)
            {
                eprintln!("bus failure end write failed: {bus_error}");
            }
            return Err(error);
        }
    };
    let decode_secs = started.elapsed().as_secs_f64();
    let transcript_text = verdict.text.clone();
    // L2: previews/drafts stay raw; seal and delivery take the custom lexicon,
    // then the Light+ floor — deterministic sentence shape, the same pass the
    // app mints at its terminal seal. Only `--raw` (≙ the Ctrl-hold literal
    // lane) promises the words untouched.
    let lexicon_text =
        codescribe_core::quality::overlay_quality::apply_custom_lexicon(&transcript_text);
    let delivered_text = if raw {
        lexicon_text
    } else {
        codescribe_core::pipeline::light_plus::apply(&lexicon_text)
    };

    if let Some(lane) = lane.as_mut()
        && let Err(error) = lane.publish_sealed(&delivered_text, &verdict.raw.segments)
    {
        eprintln!("bus seal write failed: {error}");
    }

    // A stream already carries the transcript. Only an actual final correction
    // (for example the custom lexicon) warrants a marked replacement.
    if let Some(final_output) = final_stdout(stream, &streamed_text, &delivered_text) {
        println!("{final_output}");
    }

    // Provenance to stderr, GUI-truth style.
    eprintln!(
        "engine={:?}/{:?} decode_secs={:.2} segments={} chars={} avg_logprob={} light_plus={} transcript_authority=stt_verdict",
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
        !raw,
    );

    // Every take leaves its observer card beside the source
    // (`docs/truth-contract.md`): `<file>.truth.json` records what really
    // produced this transcript. It is an OBSERVER projection of the verdict —
    // no delivery path reads it back. A write failure is a warning on stderr,
    // never a failed transcription.
    if write_truth {
        let truth = TakeTruth::from_verdict(&verdict, "CLI • Transcript", TRUTH_ENERGY_BUCKETS);
        if let Err(error) = write_truth_sidecar(file, &truth) {
            eprintln!(
                "truth sidecar write failed for {}: {error:#}",
                file.display()
            );
        }
    }

    if inspect {
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|columns| columns.parse::<usize>().ok())
            .unwrap_or(INSPECT_DEFAULT_WIDTH);
        eprint!("{}", render_inspect(&verdict, width));
    }

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

fn final_stdout(stream: bool, streamed: &str, delivered: &str) -> Option<String> {
    if !stream || streamed.is_empty() {
        return Some(delivered.to_string());
    }
    if streamed.split_whitespace().eq(delivered.split_whitespace()) {
        None
    } else {
        Some(format!("⟲ final: {delivered}"))
    }
}

/// Terminal width for `--inspect` when `$COLUMNS` is unset or not a number.
const INSPECT_DEFAULT_WIDTH: usize = 100;
/// Energy sparkline width baked into CLI `.truth.json` sidecars.
const TRUTH_ENERGY_BUCKETS: usize = 100;
/// The shared octile bar alphabet (`▁` = floor, `█` = peak).
const SPARKLINE_BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Level of one sparkline bar in the octile alphabet. Glyphs outside it (a
/// space, the 500 ms `█▓░` row's chars) read as the floor.
fn sparkline_level(bar: char) -> usize {
    SPARKLINE_BARS
        .iter()
        .position(|&candidate| candidate == bar)
        .unwrap_or(0)
}

/// Resample a bar sparkline to `width` chars, keeping the MAX level per
/// bucket: a 32 ms onset must survive aggregation, never average away.
fn resample_sparkline_max(sparkline: &str, width: usize) -> String {
    let chars: Vec<char> = sparkline.chars().collect();
    let n = chars.len();
    if n == 0 || width == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(width);
    for bucket in 0..width {
        let start = bucket * n / width;
        let end = (bucket + 1) * n / width;
        let level = if start >= end {
            sparkline_level(chars[start.min(n - 1)])
        } else {
            chars[start..end]
                .iter()
                .map(|&bar| sparkline_level(bar))
                .max()
                .unwrap_or(0)
        };
        out.push(SPARKLINE_BARS[level]);
    }
    out
}

/// `mm:ss.s` for the segment block of `--inspect`.
fn format_inspect_ts(ts: f32) -> String {
    let minutes = (ts / 60.0).floor() as u64;
    let seconds = ts - minutes as f32 * 60.0;
    format!("{minutes:02}:{seconds:04.1}")
}

/// Render the take truth under one time axis: a 5 s tick line, the segment
/// block, then the 32 ms Silero row and the energy row resampled to `width`.
/// The two sparkline rows always carry identical char length. With no VAD
/// verdict (the Apple path never runs file final, but the shape exists) the
/// fine row reports `fine: n/a` instead of inventing a clock.
fn render_inspect(
    verdict: &codescribe_core::pipeline::contracts::TranscriptionVerdict,
    width: usize,
) -> String {
    let width = width.max(20);
    let segments = &verdict.raw.segments;
    let vad = verdict.vad.as_ref();
    let fine = vad
        .map(|vad| vad.fine_sparkline.as_str())
        .filter(|sparkline| !sparkline.is_empty());
    let energy = verdict
        .raw
        .energy
        .as_ref()
        .filter(|timeline| !timeline.frames.is_empty());

    // One time axis: the longest clock the take recorded.
    let duration_secs = [
        segments.last().map(|segment| f64::from(segment.end_ts)),
        vad.map(|vad| {
            vad.fine_sparkline.chars().count() as f64 * f64::from(vad.fine_hop_ms) / 1000.0
        }),
        energy.map(|timeline| timeline.frames.len() as f64 * f64::from(timeline.hop_ms) / 1000.0),
    ]
    .into_iter()
    .flatten()
    .fold(0.0_f64, f64::max);

    // A tick every 5 s, always one at zero: ceil(duration / 5) + 1 marks.
    let tick_count = (duration_secs / 5.0).ceil() as usize + 1;
    let column = |t: f64| -> usize {
        if duration_secs <= 0.0 {
            return 0;
        }
        ((t / duration_secs) * (width - 1) as f64)
            .round()
            .min((width - 1) as f64) as usize
    };
    let mut labels = vec![' '; width];
    let mut ticks = vec!['.'; width];
    for tick in 0..tick_count {
        let col = column(tick as f64 * 5.0);
        ticks[col] = '|';
        let label = format!("{}s", tick * 5);
        // A label that would fall off the right edge shifts left instead of
        // truncating; the tick column itself never moves.
        let start = col.min(width - label.chars().count());
        for (offset, ch) in label.chars().enumerate() {
            labels[start + offset] = ch;
        }
    }

    let mut out = String::new();
    out.push_str(labels.iter().collect::<String>().trim_end());
    out.push('\n');
    out.push_str(&ticks.iter().collect::<String>());
    out.push('\n');
    for segment in segments {
        out.push_str(&format!(
            "[{}–{}] {}\n",
            format_inspect_ts(segment.start_ts),
            format_inspect_ts(segment.end_ts),
            segment.text
        ));
    }
    // Equal-length labels keep the two sparkline rows at identical char length.
    match fine {
        Some(sparkline) => {
            out.push_str("fine:   ");
            out.push_str(&resample_sparkline_max(sparkline, width));
            out.push('\n');
        }
        None => out.push_str("fine: n/a\n"),
    }
    match energy {
        Some(timeline) => {
            out.push_str("energy: ");
            out.push_str(&codescribe_core::stt::whisper::energy::sparkline(
                &timeline.frames,
                width,
            ));
            out.push('\n');
        }
        None => out.push_str("energy: n/a\n"),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_does_not_repeat_delivery_but_exposes_a_real_final_correction() {
        assert_eq!(
            final_stdout(true, "Pierwsze zdanie. Drugie.", "Pierwsze zdanie. Drugie."),
            None
        );
        assert_eq!(
            final_stdout(true, "raszt", "Rust"),
            Some("⟲ final: Rust".into())
        );
        assert_eq!(
            final_stdout(true, "", "Tekst bez segmentów"),
            Some("Tekst bez segmentów".into())
        );
        assert_eq!(
            final_stdout(false, "", "Pełny dokument"),
            Some("Pełny dokument".into())
        );
    }

    /// A file-mode verdict carrying both clocks: segments closing at 12.4 s,
    /// a 32 ms fine row (388 chunks), and a 10 ms energy row (1240 frames).
    fn inspect_verdict() -> codescribe_core::pipeline::contracts::TranscriptionVerdict {
        use codescribe_core::pipeline::contracts::{
            EnergyTimeline, RawTranscript, TranscriptSegment, TranscriptionEngineMode,
            TranscriptionEngineVerdict, TranscriptionSource, TranscriptionVerdict, VadVerdict,
        };
        let fine_sparkline: String = (0..388).map(|chunk| SPARKLINE_BARS[chunk % 8]).collect();
        TranscriptionVerdict::from_parts(
            "pierwsze drugie".to_string(),
            RawTranscript {
                text: "pierwsze drugie".to_string(),
                segments: vec![
                    TranscriptSegment {
                        text: "pierwsze".to_string(),
                        start_ts: 0.0,
                        end_ts: 2.4,
                    },
                    TranscriptSegment {
                        text: "drugie".to_string(),
                        start_ts: 2.4,
                        end_ts: 12.4,
                    },
                ],
                avg_logprob: Some(-0.2),
                energy: Some(EnergyTimeline {
                    hop_ms: 10,
                    frames: (0..1240).map(|frame| -80.0 + (frame % 50) as f32).collect(),
                    voice: Vec::new(),
                }),
                ..Default::default()
            },
            Some(VadVerdict {
                speech_pct: 61.0,
                speech_windows: 10,
                total_windows: 25,
                no_speech: false,
                no_speech_reason: None,
                sparkline: "░███".to_string(),
                fine_sparkline,
                fine_hop_ms: 32,
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        )
    }

    /// The 32 ms Silero row and the energy row render at identical char
    /// length (label included), each sparkline body exactly `width` chars.
    #[test]
    fn inspect_sparkline_rows_have_identical_length() {
        let rendered = render_inspect(&inspect_verdict(), 40);
        let fine = rendered
            .lines()
            .find(|line| line.starts_with("fine:"))
            .expect("fine row");
        let energy = rendered
            .lines()
            .find(|line| line.starts_with("energy:"))
            .expect("energy row");
        assert_eq!(fine.chars().count(), energy.chars().count());
        assert_eq!(
            fine.strip_prefix("fine:   ")
                .expect("label")
                .chars()
                .count(),
            40
        );
        assert_eq!(
            energy
                .strip_prefix("energy: ")
                .expect("label")
                .chars()
                .count(),
            40
        );
    }

    /// A 12.4 s take carries ceil(12.4 / 5) + 1 = 4 tick marks: 0s, 5s, 10s, 15s.
    #[test]
    fn inspect_tick_count_is_ceil_duration_over_five_plus_one() {
        let rendered = render_inspect(&inspect_verdict(), 50);
        assert_eq!(rendered.matches('|').count(), 4);
        assert!(rendered.lines().next().expect("labels").contains("15s"));
    }

    /// No VAD verdict (the Apple shape) reports the fine row as n/a instead
    /// of inventing a clock; the energy row still renders.
    #[test]
    fn inspect_without_vad_reports_fine_na() {
        let mut verdict = inspect_verdict();
        verdict.vad = None;
        let rendered = render_inspect(&verdict, 40);
        assert!(rendered.contains("fine: n/a"));
        assert!(rendered.lines().any(|line| line.starts_with("energy: ")));
    }

    /// Max-per-bucket resampling: a lone peak survives aggregation, and an
    /// upsampled row repeats its source chars instead of inventing data.
    #[test]
    fn resample_max_keeps_the_peak_of_each_bucket() {
        assert_eq!(resample_sparkline_max("▁▁█▁", 2), "▁█");
        assert_eq!(resample_sparkline_max("▂▄", 4), "▂▂▄▄");
        assert_eq!(resample_sparkline_max("", 8), "");
    }

    /// `--inspect` and `--no-truth` parse as file-mode flags.
    #[test]
    fn inspect_and_no_truth_flags_parse() {
        let cli = Cli::try_parse_from([
            "codescribe",
            "transcribe",
            "a.wav",
            "--inspect",
            "--no-truth",
        ])
        .expect("flags should parse");
        let Command::Transcribe {
            inspect,
            no_truth,
            files,
            ..
        } = cli.command
        else {
            panic!("`transcribe` must parse as the transcribe command");
        };
        assert!(inspect);
        assert!(no_truth);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn live_command_is_a_subcommand_not_a_file_named_live() {
        let cli = Cli::try_parse_from(["codescribe", "transcribe", "live", "--language", "pl"])
            .expect("live command should parse");
        let Command::Transcribe {
            files,
            language,
            mode,
            ..
        } = cli.command
        else {
            panic!("`transcribe live` must parse as the transcribe command");
        };
        assert!(files.is_empty());
        assert_eq!(language.as_deref(), Some("pl"));
        assert!(matches!(mode, Some(TranscribeMode::Live)));
    }

    /// The trap this cut closes: `--json` used to be rejected after `live`,
    /// because it was declared on the parent and was not global.
    #[test]
    fn json_parses_on_either_side_of_the_live_subcommand() {
        for argv in [
            ["codescribe", "transcribe", "live", "--json"],
            ["codescribe", "transcribe", "--json", "live"],
        ] {
            let cli = Cli::try_parse_from(argv).expect("--json must parse in both positions");
            let Command::Transcribe { json, mode, .. } = cli.command else {
                panic!("`transcribe live` must parse as the transcribe command");
            };
            assert!(json, "{argv:?}");
            assert!(matches!(mode, Some(TranscribeMode::Live)));
        }
    }

    /// File-only flags stay local on purpose: `transcribe live --help` must not
    /// advertise options that lane refuses.
    #[test]
    fn file_only_flags_are_refused_after_the_live_subcommand() {
        assert!(
            Cli::try_parse_from(["codescribe", "transcribe", "live", "--raw"]).is_err(),
            "--raw belongs to file mode and must not parse under `live`"
        );
    }

    #[test]
    fn lexicon_actions_parse() {
        let cli = Cli::try_parse_from(["codescribe", "lexicon", "restore", "--dry-run"])
            .expect("lexicon restore should parse");
        let Command::Lexicon { action } = cli.command else {
            panic!("`lexicon` must parse as the lexicon command");
        };
        assert!(matches!(
            action,
            LexiconAction::Restore { dry_run: true, .. }
        ));
    }

    /// Founder 2026-09-05: "to też naturalne, że powinno przejść" — a batch of
    /// wavs on the command line is the natural CLI shape, not an error.
    #[test]
    fn multiple_files_parse_as_a_batch_not_an_error() {
        let cli = Cli::try_parse_from(["codescribe", "transcribe", "a.wav", "b.wav", "c.wav"])
            .expect("multi-file should parse");
        let Command::Transcribe { files, mode, .. } = cli.command else {
            panic!("a wav batch must parse as the transcribe command");
        };
        assert_eq!(files.len(), 3);
        assert!(mode.is_none());
    }

    fn projection(
        kind: codescribe::presentation::transcript_projection::TranscriptProjectionKind,
        session: &str,
        revision: u64,
        text: &str,
    ) -> codescribe::presentation::transcript_projection::TranscriptProjection {
        codescribe::presentation::transcript_projection::TranscriptProjection {
            schema: codescribe::presentation::transcript_projection::PROJECTION_SCHEMA,
            source: None,
            kind,
            session_id: session.to_string(),
            sequence: revision,
            reducer_revision: revision,
            reducer_action: "apply_ledger_decision".into(),
            occurrence_session_id: session.to_string(),
            capture_epoch: 1,
            sample_start: 100,
            sample_end: 900,
            document_index: 0,
            rendered_text: text.to_string(),
            phase: Default::default(),
            can_paste: false,
            can_insert: false,
            can_copy: false,
            can_retranscribe: false,
            can_format: false,
            can_send_to_agent: false,
            terminal: false,
        }
    }

    /// The human canvas prints only what changed: an extending revision appends
    /// the suffix to the open line instead of reprinting the whole document.
    #[test]
    fn human_view_appends_only_the_delta_of_an_extending_revision() {
        use codescribe::presentation::transcript_projection::TranscriptProjectionKind;
        let mut view = LiveHumanView::default();
        let first = view.render(&projection(
            TranscriptProjectionKind::LiveRevision,
            "sess-a",
            1,
            "alfa",
        ));
        let second = view.render(&projection(
            TranscriptProjectionKind::LiveRevision,
            "sess-a",
            2,
            "alfa beta",
        ));
        assert_eq!(first, "alfa");
        assert_eq!(second, " beta");
    }

    /// A reducer rewrite is not an append: the canvas closes the stale line and
    /// marks the replacement explicitly instead of printing a duplicate.
    #[test]
    fn human_view_marks_a_rewrite_instead_of_duplicating_text() {
        use codescribe::presentation::transcript_projection::TranscriptProjectionKind;
        let mut view = LiveHumanView::default();
        view.render(&projection(
            TranscriptProjectionKind::LiveRevision,
            "sess-a",
            1,
            "alfa bety",
        ));
        let rewrite = view.render(&projection(
            TranscriptProjectionKind::LiveRevision,
            "sess-a",
            2,
            "alfa beta gamma",
        ));
        assert_eq!(rewrite, "\n⟲ rev 2: alfa beta gamma");
    }

    /// The seal closes the open draft line and prints a permanent block; a seal
    /// with no open line does not start with a stray newline.
    #[test]
    fn human_view_seal_closes_the_take_as_a_permanent_block() {
        use codescribe::presentation::transcript_projection::TranscriptProjectionKind;
        let mut view = LiveHumanView::default();
        view.render(&projection(
            TranscriptProjectionKind::LiveRevision,
            "sess-abcdef",
            1,
            "alfa",
        ));
        let sealed = view.render(&projection(
            TranscriptProjectionKind::TerminalSeal,
            "sess-abcdef",
            3,
            "alfa beta",
        ));
        assert_eq!(
            sealed,
            "\n⏺ sealed · session sess-abc · rev 3 · samples 100..900\nalfa beta\n\n"
        );

        let mut cold = LiveHumanView::default();
        let cold_seal = cold.render(&projection(
            TranscriptProjectionKind::TerminalSeal,
            "sess-x",
            1,
            "solo",
        ));
        assert!(!cold_seal.starts_with('\n'));
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
