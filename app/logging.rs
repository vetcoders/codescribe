//! Process-wide tracing/log initialization.
//!
//! Historically this lived in the CLI entrypoint `bin/codescribe.rs`, which was
//! deleted together with the legacy AppKit UI (commit 37efe51). The SwiftUI app
//! enters exclusively through the UniFFI bridge and never had a `main()` that
//! called it — so from that excision onward the app installed **no** tracing
//! subscriber and stopped writing `~/.codescribe/logs/codescribe.log`.
//!
//! [`init_logging`] restores that behaviour. It is safe to call from every FFI
//! entry point: a [`Once`] guard makes it idempotent, so whichever bridge object
//! Swift constructs first wins and the rest are no-ops.

use std::env;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Once;

use codescribe_core::config::Config;

/// Once guard so tracing/logging subscribers install exactly once per process.
static INIT: Once = Once::new();

/// Install the global tracing subscriber (stderr + file) and the panic hook.
///
/// Idempotent: guarded by a [`Once`], so repeated calls across FFI boundaries
/// are cheap no-ops. Production processes append to
/// `~/.codescribe/logs/codescribe.log`, honouring `RUST_LOG` (falling back to
/// legacy `LOG_LEVEL`, then `info`). Rust and XCTest harnesses are refused a
/// file sink at runtime, including integration tests where this library is
/// compiled without `cfg(test)`; they retain the stderr subscriber.
pub fn init_logging() {
    INIT.call_once(|| {
        init_tracing();
        install_panic_hook();
    });
}

/// Build and install the subscriber: a stderr layer always, plus a file layer
/// when the log file can be opened. An unopenable log file degrades to
/// stderr-only rather than losing tracing altogether.
fn init_tracing() {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, fmt};

    // Prefer `RUST_LOG`, fall back to legacy `LOG_LEVEL`.
    let filter = match env::var("RUST_LOG") {
        Ok(v) => v,
        Err(_) => match env::var("LOG_LEVEL") {
            Ok(v) => v.to_lowercase(),
            Err(_) => "info".to_string(),
        },
    };

    let stderr_layer = fmt::layer()
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true);

    let filter_layer = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));

    let file = log_file_path(&Config::config_dir(), runtime_is_test_process())
        .and_then(|path| open_file_log(&path).ok());

    if let Some(file) = file {
        let file = std::sync::Arc::new(file);
        let file_layer = fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_writer(move || (*file).try_clone().expect("Failed to clone log file"));

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

/// Resolve the production file sink. Test harnesses deliberately receive no
/// path: relying on a Makefile-exported data directory is insufficient because
/// bare `cargo test` compiles integration-test dependencies without `cfg(test)`.
fn log_file_path(config_dir: &Path, test_process: bool) -> Option<PathBuf> {
    (!test_process).then(|| config_dir.join("logs").join("codescribe.log"))
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
/// in `~/.codescribe/logs/codescribe.log`.
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
    fn test_processes_are_refused_a_file_sink() {
        let root = Path::new("/tmp/codescribe-test-logging-contract");
        assert_eq!(log_file_path(root, true), None);
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
}
