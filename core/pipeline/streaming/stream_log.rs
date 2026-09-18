//! Session stream-log sink (`CODESCRIBE_STREAM_LOG*` env contract).

use std::{fs::OpenOptions, io::Write, path::Path};

use chrono::SecondsFormat;

// ── Logging ──────────────────────────────────────────────────────────────────

/// Read a boolean stream-log flag; unset and malformed values are off.
fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Resolve where the stream log should be written, or `None` when logging is
/// off.
///
/// An explicit `CODESCRIBE_STREAM_LOG_PATH` wins outright; otherwise the
/// boolean `CODESCRIBE_STREAM_LOG` opts into the default location under the
/// config dir. Two variables rather than one so a path can be set without
/// also having to flip the switch.
pub(crate) fn stream_log_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("CODESCRIBE_STREAM_LOG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(std::path::PathBuf::from(trimmed));
        }
    }

    if env_bool("CODESCRIBE_STREAM_LOG") {
        let root = crate::config::Config::config_dir();
        return Some(root.join("stream.log"));
    }

    None
}

/// Append one RFC 3339-stamped line to the stream log, creating the file and
/// its parent directory as needed.
///
/// Newlines, carriage returns and backspaces are escaped rather than written
/// literally, which keeps the file strictly one record per line — partials
/// routinely carry control characters that would otherwise split a record.
pub(crate) fn append_to_stream_log(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let ts = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut payload = text.replace('\n', "\\n").replace('\r', "\\r");
    payload = payload.replace('\u{0008}', "\\b");
    writeln!(file, "[{}] {}", ts, payload)?;
    Ok(())
}
