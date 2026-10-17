//! A session's replay card: the inputs it ran with, written once at session
//! start beside its trail as `sessions/<session>.replay-card.json`.
//!
//! `codescribe replay` rebuilds the same `SessionConfig` from the card instead
//! of today's settings and a guessed device. The trail cannot carry this: its
//! `start` record is unique and fixed. The card holds no spoken text and no
//! secret, and its settings record reseals the session's snapshot digest
//! exactly (see `config::replay_inputs`). Writing it is diagnostic: a failure
//! is logged by kind and never touches transcription.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::streaming::SessionConfig;
use crate::audio::streaming_recorder::CaptureTurnIntent;
use crate::config::Config;
use crate::config::replay_inputs::CapturedInputsRecord;

pub const REPLAY_CARD_SCHEMA: &str = "codescribe.replay-card.v1";

#[derive(Clone, Serialize, Deserialize)]
pub struct ReplayCard {
    pub schema: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub written_at_unix_ms: u64,
    pub build: ReplayBuild,
    pub sample_rate: u32,
    /// The device the recorder opened. The session resolves its energy
    /// calibration by this name; without it no occurrence can qualify.
    pub capture_device_name: Option<String>,
    pub language: Option<String>,
    pub utterance_silence_sec: Option<f32>,
    pub capture_turn: CaptureTurnIntent,
    /// `Layer1Decision::receipt_token` at session start.
    pub layer1: String,
    /// A live formatter executor was bound. Its output is LLM text, which a
    /// replay does not reproduce; replay parity is the reducer document.
    pub live_formatter: bool,
    pub lifecycle_events: bool,
    /// The stop path hands the session its finalized WAV archive.
    pub terminal_archive: bool,
    pub settings_digest: String,
    pub settings: CapturedInputsRecord,
}

/// Which build ran the session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayBuild {
    pub core_version: String,
    /// `CSBuildCommit` of the app bundle that ran the session. `None` outside
    /// a bundle (CLI, tests) or when the bundle carries no stamp.
    pub app_commit: Option<String>,
}

impl ReplayBuild {
    pub fn current() -> Self {
        static APP_COMMIT: OnceLock<Option<String>> = OnceLock::new();
        Self {
            core_version: env!("CARGO_PKG_VERSION").to_string(),
            app_commit: APP_COMMIT
                .get_or_init(|| {
                    let exe = std::env::current_exe().ok()?;
                    let contents = exe.parent()?.parent()?;
                    let plist = fs::read_to_string(contents.join("Info.plist")).ok()?;
                    plist_string(&plist, "CSBuildCommit")
                })
                .clone(),
        }
    }
}

/// The `<string>` value following `<key>name</key>` in an XML plist.
fn plist_string(plist: &str, name: &str) -> Option<String> {
    let after_key = &plist[plist.find(&format!("<key>{name}</key>"))?..];
    let value = &after_key[after_key.find("<string>")? + "<string>".len()..];
    let value = value[..value.find("</string>")?].trim();
    (!value.is_empty()).then(|| value.to_string())
}

impl ReplayCard {
    pub fn of_session(config: &SessionConfig) -> Self {
        let settings = &config.runtime_settings;
        Self {
            schema: REPLAY_CARD_SCHEMA.to_string(),
            session_id: config.session_id.clone(),
            capture_epoch: config.capture_epoch,
            written_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as u64)
                .unwrap_or_default(),
            build: ReplayBuild::current(),
            sample_rate: config.sample_rate,
            capture_device_name: config.capture_device_name.clone(),
            language: config.language.clone(),
            utterance_silence_sec: config.utterance_silence_sec,
            capture_turn: config.capture_turn,
            layer1: config.layer1.receipt_token(),
            live_formatter: config.live_formatting_agent.is_some(),
            lifecycle_events: config.lifecycle_events.is_some(),
            terminal_archive: config.terminal_audio.is_some(),
            settings_digest: settings.digest().as_str().to_string(),
            settings: CapturedInputsRecord::redacted(settings.captured_inputs()),
        }
    }
}

/// `sessions/<session>.replay-card.json`, under the trail's session alphabet.
pub fn card_path(root: &Path, session: &str) -> io::Result<PathBuf> {
    Ok(super::trail::trail_path(root, session)?
        .with_file_name(format!("{session}.replay-card.json")))
}

/// Write the session's card off the capture task.
pub(crate) fn record(config: &SessionConfig) {
    let card = ReplayCard::of_session(config);
    let root = Config::config_dir();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = write_in(&root, &card) {
            tracing::warn!(kind = ?error.kind(), "replay card unavailable");
        }
    });
}

fn write_in(root: &Path, card: &ReplayCard) -> io::Result<()> {
    let path = card_path(root, &card.session_id)?;
    fs::create_dir_all(root.join("sessions"))?;
    let mut options = OpenOptions::new();
    // A reused session id keeps the first card; never follow a symlink.
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let mut bytes = serde_json::to_vec_pretty(card).map_err(io::Error::other)?;
    bytes.push(b'\n');
    file.write_all(&bytes)
}

/// The session's card, `None` when the session predates cards.
pub fn load_in(root: &Path, session: &str) -> io::Result<Option<ReplayCard>> {
    let bytes = match fs::read(card_path(root, session)?) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let card: ReplayCard = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if card.schema != REPLAY_CARD_SCHEMA || card.session_id != session {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "replay card schema or session mismatch",
        ));
    }
    Ok(Some(card))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_string_reads_the_build_stamp() {
        let plist = "<dict>\n\t<key>CSBuildCommit</key>\n\t<string>b2557e401</string>\n\t<key>CSBuiltAt</key>\n\t<string>2026-10-17T17:40:00Z</string>\n</dict>";
        assert_eq!(
            plist_string(plist, "CSBuildCommit").as_deref(),
            Some("b2557e401")
        );
        assert_eq!(plist_string(plist, "CSBuildNumber"), None);
        assert_eq!(
            plist_string(
                "<key>CSBuildCommit</key><string> </string>",
                "CSBuildCommit"
            ),
            None
        );
    }

    #[test]
    fn card_path_sits_beside_the_trail_and_refuses_unsafe_ids() {
        let root = Path::new("/workspace/codescribe-data");
        assert_eq!(
            card_path(root, "agent-channel-0-9943d3cb").unwrap(),
            root.join("sessions/agent-channel-0-9943d3cb.replay-card.json")
        );
        assert!(card_path(root, "../../escape").is_err());
    }
}
