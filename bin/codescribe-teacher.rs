//! Legacy entry point for `codescribe teach`.
//!
//! Kept so existing scripts keep working; the surface lives in
//! `codescribe::cli::teacher` so both spellings run identical code.

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "codescribe-teacher",
    about = "Teacher: live×whisper×human → Needs attention → lexicon"
)]
struct Cli {
    #[command(subcommand)]
    cmd: codescribe::cli::teacher::TeacherCommand,
}

fn main() -> anyhow::Result<()> {
    codescribe::cli::teacher::run(Cli::parse().cmd)
}
