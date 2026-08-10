//! One-shot operator tool: migrate plaintext MCP env secrets to Keychain.
//!
//! ```text
//! cargo run -p codescribe-core --example migrate_mcp_secrets -- --dry-run
//! cargo run -p codescribe-core --example migrate_mcp_secrets -- --apply
//! ```
//!
//! Never prints secret values.

use std::env;
use std::process::ExitCode;

use codescribe_core::mcp::{
    default_mcp_config_path, format_secret_migration_report, migrate_plaintext_env_secrets,
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let dry_run = !args.iter().any(|a| a == "--apply");
    let path = match default_mcp_config_path() {
        Ok(p) => p,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    match migrate_plaintext_env_secrets(&path, dry_run) {
        Ok(report) => {
            println!("target: {}", path.display());
            println!("{}", format_secret_migration_report(&report));
            if dry_run {
                println!("(dry-run — pass --apply to write backup + rewritten mcp.json)");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
