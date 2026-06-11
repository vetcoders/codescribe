//! Session stream-log sink (`CODESCRIBE_STREAM_LOG*` env contract).

use std::{fs::OpenOptions, io::Write, path::Path};

use chrono::SecondsFormat;

use super::tuning::env_bool;

// ── Logging ──────────────────────────────────────────────────────────────────

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
