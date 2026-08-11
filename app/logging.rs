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
use std::path::PathBuf;
use std::sync::Once;

/// Once guard so tracing/logging subscribers install exactly once per process.
static INIT: Once = Once::new();

/// Install the global tracing subscriber (stderr + file) and the panic hook.
///
/// Idempotent: guarded by a [`Once`], so repeated calls across FFI boundaries
/// are cheap no-ops. Writes to `~/.codescribe/logs/codescribe.log` (append),
/// honouring `RUST_LOG` (falling back to legacy `LOG_LEVEL`, then `info`).
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

    let log_path = resolved_log_path();
    let log_dir = log_path
        .parent()
        .expect("codescribe log path always has a parent");
    let _ = std::fs::create_dir_all(log_dir);

    let stderr_layer = fmt::layer()
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true);

    let filter_layer = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path);

    if let Ok(file) = file {
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

/// Resolve the file sink independently from subscriber installation so tests
/// can prove they never target the operator's production log.
fn resolved_log_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".codescribe")
        .join("logs")
        .join("codescribe.log")
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

    #[test]
    fn fleet_red_test_logging_isolated() {
        let production = PathBuf::from(env::var("HOME").expect("HOME must be set"))
            .join(".codescribe")
            .join("logs")
            .join("codescribe.log");
        assert_ne!(
            resolved_log_path(),
            production,
            "test logger initialization must resolve inside test-local data, never production"
        );
    }
}
