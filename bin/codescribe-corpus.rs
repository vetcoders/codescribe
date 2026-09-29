//! Legacy entry point for `codescribe corpus`.
//!
//! Kept so existing scripts keep working; the surface lives in
//! `codescribe::cli::corpus` so both spellings run identical code.

use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "codescribe-corpus",
    about = "Private corpus census and production-overlay replay",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: codescribe::cli::corpus::CorpusCommand,
}

fn main() -> ExitCode {
    // SAFETY: first executable statement, before Clap parsing, runtime
    // construction or thread creation. Corpus tooling must be unable to read,
    // write or prompt for the operator's production Keychain even if a future
    // replay dependency unexpectedly reaches Config/secret code.
    unsafe {
        std::env::set_var("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    }
    codescribe::cli::corpus::main_with(
        Cli::parse().command,
        codescribe::cli::corpus::Invocation::legacy_binary(),
    )
}
