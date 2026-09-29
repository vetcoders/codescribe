//! Legacy entry point for `codescribe daemon`.
//!
//! Kept so existing scripts and launchd jobs keep working; the surface lives in
//! `codescribe::cli::daemon` so both spellings run identical code.

use clap::Parser;

#[derive(Parser)]
#[command(name = "qube-daemon")]
#[command(version)]
#[command(about = "Run the self-improving quality loop (report + regression + tuning)")]
struct Cli {
    #[command(flatten)]
    args: codescribe::cli::daemon::DaemonArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    codescribe::cli::daemon::run(Cli::parse().args).await
}
