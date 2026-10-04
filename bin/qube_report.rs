//! Legacy entry point for `codescribe report`.
//!
//! Kept so existing scripts and muscle memory keep working. The flags, the
//! dispatch and the output all live in `codescribe::cli::report`, so this
//! spelling can never drift from the subcommand.

use clap::Parser;

#[derive(Parser)]
#[command(name = "qube-report")]
#[command(version)]
#[command(about = "Generate a quality report for Codescribe transcriptions")]
struct Cli {
    #[command(flatten)]
    args: codescribe::cli::report::ReportArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    codescribe::cli::report::run(Cli::parse().args).await
}
