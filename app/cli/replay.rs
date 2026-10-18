//! `codescribe replay`: run a recorded take through the production session
//! again and print the document the overlay would show for it.
//!
//! Inputs come from the take's replay card (`sessions/<id>.replay-card.json`),
//! so a replay runs the take's own settings, capture device, capture turn and
//! terminal archive. Takes recorded before cards resolve each input to the
//! nearest honest value, and the receipt names every substitution. Replay
//! keeps live pacing: a 60 s take needs about 60 s per run.
//!
//! `--ledger on|off` and `--device NAME` replay the take as a what-if. The
//! receipt lists them under `scenario`, and their evidence gets its own
//! session id, so a plain replay of the same take is never overwritten.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use codescribe_core::audio::streaming_recorder::{ReplayInputReceipt, ReplayScenario};
use codescribe_core::config::Config;
use codescribe_core::pipeline::replay_card::{self, ReplayBuild};
use serde::Serialize;

use crate::controller::production_replay::{RecordedTakeRun, replay_recorded_take};

pub const REPLAY_RECEIPT_SCHEMA: &str = "codescribe.replay-receipt.v1";

#[derive(clap::Args, Debug)]
pub struct ReplayArgs {
    /// Session id, or a retained audio file of the take
    pub take: String,
    /// Replay this many times; the receipt says whether the runs agree
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=20))]
    pub runs: u32,
    /// Print the full receipt as JSON on stdout instead of the text
    #[arg(long)]
    pub json: bool,
    /// Scenario: replay as if the take had been captured on this device
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
    /// Scenario: replay with seal-ledger admission on or off
    #[arg(long, value_name = "on|off", value_parser = on_off)]
    pub ledger: Option<bool>,
}

fn on_off(value: &str) -> Result<bool, String> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("expected on or off, got {other}")),
    }
}

#[derive(Serialize)]
struct ReplayerBuild {
    core_version: &'static str,
    commit: &'static str,
}

#[derive(Serialize)]
struct ReplayReceipt {
    schema: &'static str,
    recorded_session: Option<String>,
    card: bool,
    recorded_build: Option<ReplayBuild>,
    replayer_build: ReplayerBuild,
    /// Both commits are known and name the same build.
    same_build: Option<bool>,
    inputs: Vec<ReplayInputReceipt>,
    /// Inputs whose replay value is not the take's own.
    divergences: Vec<&'static str>,
    /// Switches that changed the take on purpose; non-empty means a what-if,
    /// not a parity check.
    scenario: Vec<String>,
    runs: Vec<RecordedTakeRun>,
    runs_agree: bool,
}

pub async fn run(args: ReplayArgs) -> Result<()> {
    let root = Config::config_dir();
    let (session, audio) = resolve_take(&root, &args.take)?;
    let card = match &session {
        Some(session) => replay_card::load_in(&root, session)
            .map_err(|error| anyhow!("replay card for {session} is unreadable: {error}"))?,
        None => None,
    };
    let scenario = ReplayScenario {
        capture_device: args.device.clone(),
        seal_ledger: args.ledger,
    };
    let mut switches = Vec::new();
    if let Some(enabled) = args.ledger {
        switches.push(format!("--ledger {}", if enabled { "on" } else { "off" }));
    }
    if let Some(device) = &args.device {
        switches.push(format!("--device {device}"));
    }
    let mut runs = Vec::new();
    let mut inputs = Vec::new();
    let mut recorded_build = None;
    for run in 1..=args.runs {
        if !args.json {
            eprintln!("replay run {run}/{} at live pace", args.runs);
        }
        let replay =
            replay_recorded_take(session.clone(), card.clone(), &audio, run, scenario.clone())
                .await?;
        inputs = replay.inputs;
        recorded_build = replay.recorded_build;
        runs.push(replay.run);
    }
    let replayer_build = ReplayerBuild {
        core_version: env!("CARGO_PKG_VERSION"),
        commit: env!("CODESCRIBE_BUILD_COMMIT"),
    };
    let same_build = recorded_build
        .as_ref()
        .and_then(|build| build.app_commit.as_deref())
        .filter(|_| replayer_build.commit != "unknown")
        .map(|recorded| same_commit(recorded, replayer_build.commit));
    let receipt = ReplayReceipt {
        schema: REPLAY_RECEIPT_SCHEMA,
        card: card.is_some(),
        recorded_session: session,
        recorded_build,
        replayer_build,
        same_build,
        divergences: inputs
            .iter()
            .filter(|input| input.reason.is_some())
            .map(|input| input.input)
            .collect(),
        scenario: switches,
        inputs,
        runs_agree: runs.windows(2).all(|pair| pair[0].text == pair[1].text),
        runs,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
        return Ok(());
    }
    eprintln!(
        "session {} · card {} · recorded build {} · replayer {}{}",
        receipt.recorded_session.as_deref().unwrap_or("unknown"),
        if receipt.card { "yes" } else { "no" },
        receipt
            .recorded_build
            .as_ref()
            .and_then(|build| build.app_commit.as_deref())
            .unwrap_or("unknown"),
        receipt.replayer_build.commit,
        match receipt.same_build {
            Some(false) => " (different build)",
            _ => "",
        }
    );
    if !receipt.scenario.is_empty() {
        eprintln!("scenario (not parity): {}", receipt.scenario.join(" "));
    }
    for input in receipt.inputs.iter().filter(|input| input.reason.is_some()) {
        eprintln!(
            "  {}: {} ({})",
            input.input,
            input.effective.as_deref().unwrap_or("none"),
            input.reason.as_deref().unwrap_or_default()
        );
    }
    if receipt.runs.len() > 1 {
        eprintln!(
            "runs: {} · {}",
            receipt.runs.len(),
            if receipt.runs_agree {
                "identical text"
            } else {
                "text differs between runs"
            }
        );
    }
    if let Some(last) = receipt.runs.last() {
        eprintln!("evidence: {}", last.evidence_dir.display());
        println!("{}", last.text);
    }
    Ok(())
}

/// A take is named by its session id or by a retained audio file. A file is
/// matched to its session through the retention receipt that owns it.
fn resolve_take(root: &Path, take: &str) -> Result<(Option<String>, PathBuf)> {
    let path = Path::new(take);
    if path.is_file() {
        #[cfg(unix)]
        let session =
            codescribe_core::state::history::audio_retention::session_for_file(root, path);
        #[cfg(not(unix))]
        let session = None;
        return Ok((session, path.to_path_buf()));
    }
    if replay_card::card_path(root, take).is_err() {
        bail!("not an audio file or a session id: {take}");
    }
    let audio = root.join("sessions").join(format!("{take}.wav"));
    if !audio.is_file() {
        bail!("session {take} has no retained audio");
    }
    Ok((Some(take.to_string()), audio))
}

/// Short commits of different lengths name one build when one prefixes the other.
fn same_commit(recorded: &str, replayer: &str) -> bool {
    let recorded = recorded.trim_end_matches("-dirty");
    let length = recorded.len().min(replayer.len());
    length >= 7 && recorded[..length] == replayer[..length]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_commits_of_one_build_match_across_lengths() {
        assert!(same_commit("b2557e401", "b2557e40"));
        assert!(same_commit("b2557e401-dirty", "b2557e401"));
        assert!(!same_commit("b2557e401", "6e4306040"));
        assert!(!same_commit("b25", "b25"));
    }

    #[test]
    fn a_take_name_that_is_neither_file_nor_session_is_refused() {
        let root = Path::new("/workspace/codescribe-data");
        assert!(resolve_take(root, "../escape").is_err());
        let missing = resolve_take(root, "0123456789abcdef").unwrap_err();
        assert!(missing.to_string().contains("no retained audio"));
    }
}
