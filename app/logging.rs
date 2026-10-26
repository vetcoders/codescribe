//! Process-wide tracing/log initialization.
//!
//! Historically this lived in the CLI entrypoint `bin/codescribe.rs`, which was
//! deleted together with the legacy AppKit UI (commit 37efe51). The SwiftUI app
//! enters exclusively through the UniFFI bridge and never had a `main()` that
//! called it — so from that excision onward the app installed **no** tracing
//! subscriber and stopped writing its log file, now
//! `~/.codescribe/data.noindex/logs/codescribe.log`.
//!
//! [`init_logging`] restores that behaviour. It is safe to call from every FFI
//! entry point: a [`Once`] guard makes it idempotent, so whichever bridge object
//! Swift constructs first wins and the rest are no-ops.

use std::env;
use std::fs::File;
use std::io::{self, BufRead as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, PoisonError};

use codescribe_core::config::{Config, churn};

/// Once guard so tracing/logging subscribers install exactly once per process.
static INIT: Once = Once::new();

/// Size at which the app rolls `codescribe.log`; a generation can exceed it
/// by the one line that crossed it.
const LOG_ROLL_BYTES: u64 = 200 * 1024 * 1024;

/// Files kept: `codescribe.log` and `.1` to `.4`, about 1 GB in all.
const LOG_GENERATIONS: usize = 5;

/// What happens to a log found over the limit at app start (Founder
/// decision pending, W2-08 D1). `false` rolls it whole to `.1`, where it
/// ages out with later rolls; `true` keeps only its last [`LOG_ROLL_BYTES`],
/// so no generation exceeds the limit and the rest is freed.
const TRIM_OVERSIZED_LOG_AT_START: bool = false;

/// Who installs the subscriber. The app (every FFI entry) owns its data
/// folder, the startup data move and the log's rotation. The CLI only
/// appends to a log the app already keeps: `bus-demux` starts it for every
/// bus publication, about 2,400 times on 2026-10-26.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogOwner {
    App,
    Cli,
}

/// Install the global tracing subscriber (stderr + file) and the panic hook.
///
/// Idempotent: guarded by a [`Once`], so repeated calls across FFI boundaries
/// are cheap no-ops. Production processes append to
/// `~/.codescribe/data.noindex/logs/codescribe.log` (`Config::churn_dir`, which
/// Spotlight does not index), honouring `RUST_LOG` (falling back to
/// legacy `LOG_LEVEL`, then `info`). Rust and XCTest harnesses are refused a
/// file sink at runtime, including integration tests where this library is
/// compiled without `cfg(test)`; they retain the stderr subscriber.
pub fn init_logging() {
    init(LogOwner::App, "info");
}

/// The CLI's subscriber, with the caller's default filter when neither
/// `RUST_LOG` nor `LOG_LEVEL` is set. The CLI uses `warn`: a file
/// transcript that lost a span must say so on stderr without drowning the
/// user in `info` chatter, and without the CLI silently dropping every engine
/// warning (it installed no subscriber at all before 2026-09-06). It moves
/// no app data and creates no log folder: a CLI that made
/// `data.noindex/logs` before the app's first move would leave both
/// locations present, and the app would refuse the move on every start.
pub fn init_cli_logging(default_filter: &str) {
    init(LogOwner::Cli, default_filter);
}

fn init(owner: LogOwner, default_filter: &str) {
    INIT.call_once(|| {
        let test_process = runtime_is_test_process();
        // The high-churn move runs before the file sink opens: the sink's
        // folder is one of the moved entries. Only the app moves its data;
        // test harnesses get no file sink and move nothing.
        let churn_moves =
            (owner == LogOwner::App && !test_process).then(churn::migrate_churn_data_at_startup);
        init_tracing(owner, default_filter, test_process);
        report_churn_moves(churn_moves.as_deref().unwrap_or_default());
        install_panic_hook();
    });
}

/// Log the startup move into `data.noindex` once the subscriber exists. A
/// refusal is an error on every start until someone resolves it by hand:
/// both locations are kept and nothing is merged.
fn report_churn_moves(moves: &[churn::ChurnMove]) {
    for item in moves {
        let (from, to) = (item.from.display(), item.to.display());
        match &item.outcome {
            churn::ChurnMoveOutcome::NothingToMove | churn::ChurnMoveOutcome::AlreadyMigrated => {}
            churn::ChurnMoveOutcome::Moved => {
                tracing::info!(target: "churn_data", %from, %to, "moved high-churn data into data.noindex");
            }
            churn::ChurnMoveOutcome::RefusedBothPresent => tracing::error!(
                target: "churn_data",
                %from,
                %to,
                "high-churn data exists at both locations; nothing moved, merge by hand"
            ),
            churn::ChurnMoveOutcome::RefusedCrossVolume => tracing::error!(
                target: "churn_data",
                %from,
                %to,
                "high-churn data is on another volume; not copied, move it by hand"
            ),
            churn::ChurnMoveOutcome::Failed(error) => tracing::error!(
                target: "churn_data",
                %from,
                %to,
                %error,
                "moving high-churn data into data.noindex failed; nothing removed"
            ),
        }
    }
}

/// Build and install the subscriber: a stderr layer always, plus a file layer
/// when the log file can be opened. An unopenable log file degrades to
/// stderr-only rather than losing tracing altogether.
fn init_tracing(owner: LogOwner, default_filter: &str, test_process: bool) {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, fmt};

    // Prefer `RUST_LOG`, fall back to legacy `LOG_LEVEL`, then the caller's default.
    let filter = match env::var("RUST_LOG") {
        Ok(v) => v,
        Err(_) => match env::var("LOG_LEVEL") {
            Ok(v) => v.to_lowercase(),
            Err(_) => default_filter.to_string(),
        },
    };

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true);

    let filter_layer = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));

    let file = log_file_path(&Config::churn_dir(), test_process)
        .and_then(|path| RollingLog::open(&path, owner).ok());

    if let Some(file) = file {
        let file_layer = fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_writer(file);

        let _ = tracing_subscriber::registry()
            .with(filter_layer)
            .with(stderr_layer)
            .with(file_layer)
            .try_init();
    } else {
        let _ = tracing_subscriber::registry()
            .with(filter_layer)
            .with(stderr_layer)
            .try_init();
    }
}

/// Resolve the production file sink under the high-churn folder. Test
/// harnesses deliberately receive no path: relying on a Makefile-exported data
/// directory is insufficient because bare `cargo test` compiles
/// integration-test dependencies without `cfg(test)`.
fn log_file_path(churn_dir: &Path, test_process: bool) -> Option<PathBuf> {
    (!test_process).then(|| churn_dir.join("logs").join("codescribe.log"))
}

/// Open the production log with append semantics, creating only its parent.
fn open_file_log(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

/// When and how far the app rolls its log.
#[derive(Debug, Clone, Copy)]
struct RollPolicy {
    bytes: u64,
    generations: usize,
}

impl RollPolicy {
    const APP: Self = Self {
        bytes: LOG_ROLL_BYTES,
        generations: LOG_GENERATIONS,
    };
}

/// The file sink: one append handle shared by every line. The app's sink
/// rolls by size, so no burst can fill the disk: on 2026-10-22 one
/// refinement loop wrote about 10 GB in 41 minutes into a log with no
/// limit. The CLI's sink only appends.
struct RollingLog {
    path: PathBuf,
    roll: Option<RollPolicy>,
    state: Mutex<RollingState>,
}

struct RollingState {
    file: File,
    /// Bytes in the active generation as this process counts them.
    written: u64,
}

impl RollingLog {
    fn open(path: &Path, owner: LogOwner) -> io::Result<Self> {
        match owner {
            LogOwner::App => Self::open_rolling(path, RollPolicy::APP, TRIM_OVERSIZED_LOG_AT_START),
            LogOwner::Cli => Self::open_append_only(path),
        }
    }

    /// The app creates the folder and rolls a log already over the limit
    /// before its first line.
    fn open_rolling(path: &Path, policy: RollPolicy, trim_oversized: bool) -> io::Result<Self> {
        let size = std::fs::metadata(path).map_or(0, |found| found.len());
        if size > policy.bytes {
            roll_generations(path, policy.generations)?;
            if trim_oversized {
                keep_tail(&generation_path(path, 1), policy.bytes)?;
            }
        }
        let file = open_file_log(path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            roll: Some(policy),
            state: Mutex::new(RollingState { file, written }),
        })
    }

    /// The CLI appends only to a folder the app made; opening fails when it
    /// is missing, and the CLI keeps its stderr subscriber.
    fn open_append_only(path: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            roll: None,
            state: Mutex::new(RollingState { file, written: 0 }),
        })
    }

    /// Append one formatted event, then roll when the generation is full.
    /// A failed roll keeps the current file and tries again a generation
    /// later. Nothing here may log through `tracing`: this is the
    /// subscriber's own writer.
    fn append(&self, line: &[u8]) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.file.write_all(line)?;
        state.written = state.written.saturating_add(line.len() as u64);
        if let Some(policy) = self.roll
            && state.written >= policy.bytes
        {
            match roll_generations(&self.path, policy.generations)
                .and_then(|()| open_file_log(&self.path))
            {
                Ok(file) => state.file = file,
                Err(error) => eprintln!("codescribe: log roll failed: {error}"),
            }
            state.written = 0;
        }
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RollingLog {
    type Writer = RollingWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RollingWriter(self)
    }
}

/// The fmt layer formats a whole event and hands it over in one write, so
/// a roll never splits a line.
struct RollingWriter<'a>(&'a RollingLog);

impl io::Write for RollingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.append(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `codescribe.log.<index>` beside `path`.
fn generation_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

/// Shift `.1` to `.2` and so on, then `path` to `.1`. The rename onto the
/// last kept generation replaces it, so nothing is copied and at most
/// `generations` files remain. A process still holding the old file keeps
/// appending to `.1`.
fn roll_generations(path: &Path, generations: usize) -> io::Result<()> {
    for index in (1..generations.saturating_sub(1)).rev() {
        let from = generation_path(path, index);
        match std::fs::rename(from, generation_path(path, index + 1)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    std::fs::rename(path, generation_path(path, 1))
}

/// Rewrite `path` to its last `bytes`, starting at a line boundary, through
/// a sibling file renamed over it.
fn keep_tail(path: &Path, bytes: u64) -> io::Result<()> {
    let mut source = File::open(path)?;
    let len = source.metadata()?.len();
    if len <= bytes {
        return Ok(());
    }
    source.seek(io::SeekFrom::Start(len - bytes))?;
    let mut reader = io::BufReader::new(source);
    // The cut line is incomplete; the kept tail starts after it.
    reader.read_until(b'\n', &mut Vec::new())?;
    let mut staging = path.as_os_str().to_os_string();
    staging.push(".trim");
    let staging = PathBuf::from(staging);
    let mut kept = File::create(&staging)?;
    io::copy(&mut reader, &mut kept)?;
    std::fs::rename(&staging, path)
}

/// Detect harnesses from runtime identity rather than only `cfg(test)`.
///
/// Cargo places unit- and integration-test executables under a `deps`
/// directory. XCTest supplies a configuration variable or an `.xctest`
/// argument even though the hosted Rust library is a normal production build.
fn runtime_is_test_process() -> bool {
    if cfg!(test)
        || env::var_os("XCTestConfigurationFilePath").is_some()
        || env::var_os("XCTestBundlePath").is_some()
        || env::args_os().any(|arg| arg.to_string_lossy().contains(".xctest"))
    {
        return true;
    }

    env::current_exe().is_ok_and(|exe| {
        exe.parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "deps")
    })
}

/// Install a global panic hook that logs every panic through `tracing` before
/// the process unwinds or aborts.
///
/// This is the only diagnostic that survives `panic="abort"` in the release
/// profile: `std::panic::set_hook` runs the hook BEFORE the abort, so even a
/// panic crossing an `extern "C"` boundary — where `catch_unwind` is useless —
/// leaves a symbolizable trace (payload + location + thread name + backtrace)
/// in `~/.codescribe/data.noindex/logs/codescribe.log`.
///
/// MUST be installed AFTER `init_tracing()` (so a subscriber exists) and BEFORE
/// the first task/thread is spawned, otherwise early panics would be silent.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // Extract a human-readable payload (panic message).
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());

        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());

        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>").to_string();

        let backtrace = std::backtrace::Backtrace::force_capture();

        tracing::error!(
            target: "panic",
            thread = %thread_name,
            location = %location,
            "PANIC: {message}\nbacktrace:\n{backtrace}"
        );
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn warning_log_keeps_stdout_clean() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "logging::tests::logging_subprocess_witness",
                "--nocapture",
            ])
            .env("RUST_LOG", "warn")
            .output()
            .unwrap();
        assert!(result.status.success());
        let marker = "codescribe-log-channel-witness";
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(marker),
            "warning missing from stderr"
        );
        assert!(
            !String::from_utf8_lossy(&result.stdout).contains(marker),
            "warning polluted transcript stdout"
        );
    }

    #[test]
    fn logging_subprocess_witness() {
        init_cli_logging("warn");
        tracing::warn!("codescribe-log-channel-witness");
    }

    #[test]
    fn test_processes_are_refused_a_file_sink() {
        let root = Path::new("/tmp/codescribe-test-logging-contract");
        assert_eq!(log_file_path(root, true), None);
    }

    /// The production sink lives in the high-churn folder, which Spotlight
    /// does not index, never directly under the data root.
    #[test]
    fn production_log_sink_resolves_under_data_noindex() {
        let path = log_file_path(&Config::churn_dir(), false).expect("production file sink");
        assert_eq!(
            path,
            Config::config_dir().join("data.noindex/logs/codescribe.log")
        );
    }

    #[test]
    fn production_logging_keeps_canonical_append_semantics() {
        let root = tempfile::tempdir().expect("create production logging fixture");
        let path = log_file_path(root.path(), false).expect("production file sink");
        assert_eq!(path, root.path().join("logs/codescribe.log"));

        writeln!(open_file_log(&path).expect("open first writer"), "first")
            .expect("write first record");
        writeln!(open_file_log(&path).expect("open append writer"), "second")
            .expect("write second record");

        assert_eq!(
            std::fs::read_to_string(path).expect("read production log fixture"),
            "first\nsecond\n"
        );
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    fn append(log: &RollingLog, line: &str) {
        use tracing_subscriber::fmt::MakeWriter as _;
        log.make_writer().write_all(line.as_bytes()).unwrap();
    }

    /// One loop wrote about 10 GB in 41 minutes on 2026-10-22. The app's
    /// sink rolls by size and keeps a fixed number of generations.
    #[test]
    fn the_app_log_rolls_by_size_and_keeps_its_generations() {
        let root = tempfile::tempdir().expect("log fixture");
        let path = root.path().join("logs/codescribe.log");
        let policy = RollPolicy {
            bytes: 64,
            generations: 3,
        };
        let log = RollingLog::open_rolling(&path, policy, false).unwrap();
        for index in 0..20 {
            append(&log, &format!("line {index:02} padded to thirty b\n"));
        }
        drop(log);

        let line = |index: usize| format!("line {index:02} padded to thirty b\n");
        assert_eq!(read(&path), line(18) + &line(19));
        let older = line(15) + &line(16) + &line(17);
        assert_eq!(read(&generation_path(&path, 1)), older);
        let oldest = line(12) + &line(13) + &line(14);
        assert_eq!(read(&generation_path(&path, 2)), oldest);
        assert!(!generation_path(&path, 3).exists(), "three in all");
    }

    #[test]
    fn an_oversized_log_found_at_start_rolls_whole_by_default() {
        let root = tempfile::tempdir().expect("log fixture");
        let logs = root.path().join("logs");
        let path = logs.join("codescribe.log");
        std::fs::create_dir_all(&logs).unwrap();
        let old = "old line padded to thirty byte\n".repeat(10);
        std::fs::write(&path, &old).expect("oversized log");
        let policy = RollPolicy {
            bytes: 64,
            generations: 3,
        };
        let log = RollingLog::open_rolling(&path, policy, false).unwrap();
        append(&log, "new\n");
        assert_eq!(read(&generation_path(&path, 1)), old);
        assert_eq!(read(&path), "new\n");
    }

    /// W2-08 D1 (a): with trimming on, an oversized log keeps only its last
    /// generation's worth of whole lines.
    #[test]
    fn an_oversized_log_keeps_only_its_tail_when_trimming() {
        let root = tempfile::tempdir().expect("log fixture");
        let logs = root.path().join("logs");
        let path = logs.join("codescribe.log");
        std::fs::create_dir_all(&logs).unwrap();
        let line = |index: usize| format!("old line {index} padded to thirty b\n");
        let old: String = (0..10).map(line).collect();
        std::fs::write(&path, &old).expect("oversized log");
        let policy = RollPolicy {
            bytes: 64,
            generations: 3,
        };
        RollingLog::open_rolling(&path, policy, true).unwrap();
        assert_eq!(read(&generation_path(&path, 1)), line(8) + &line(9));
        assert_eq!(read(&path), "");
    }

    /// The CLI starts for every bus publication: it appends to the app's log
    /// when the app's folder exists, and never creates the folder or rolls.
    #[test]
    fn the_cli_appends_without_creating_the_folder_or_rolling() {
        let root = tempfile::tempdir().expect("log fixture");
        let logs = root.path().join("logs");
        let path = logs.join("codescribe.log");
        assert!(RollingLog::open(&path, LogOwner::Cli).is_err());
        assert!(!logs.exists(), "the CLI made no log folder");

        std::fs::create_dir_all(&logs).unwrap();
        let log = RollingLog::open(&path, LogOwner::Cli).unwrap();
        assert!(log.roll.is_none());
        append(&log, "cli line\n");
        assert_eq!(read(&path), "cli line\n");
        assert!(!generation_path(&path, 1).exists());
    }
}
