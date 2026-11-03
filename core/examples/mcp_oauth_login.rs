//! Sign a remote MCP connector in with OAuth from the terminal.
//!
//! ```text
//! cargo run -p codescribe-core --example mcp_oauth_login -- \
//!     --name mega-challenge --url https://hq.mega.dev/mcp [--discover-only] [--no-open]
//! ```
//!
//! - `--discover-only` stops after the 401 challenge and metadata discovery:
//!   no client is registered, no browser opens, nothing is written.
//! - default: also registers a client, opens the browser on the consent page
//!   and waits for the redirect on a loopback port. The human signs in; on
//!   success the credential lands in the Keychain and the connector entry in
//!   `mcp.json` is created or switched to `auth_type: "oauth"`.
//! - `--no-open` prints the URL instead of opening it.
//! - `--client-id ID` skips dynamic registration for servers that need one.
//!
//! The listener lives only while this process waits, so a login interrupted
//! mid-browser is resumed by running the command again.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use codescribe_core::mcp::oauth::{
    BearerChallenge, LoginOptions, begin_login, credential_account, discover, http_client,
    probe_challenge,
};
use codescribe_core::mcp::{McpServerSpec, add_server, list_servers, update_server};

struct Args {
    name: String,
    url: String,
    discover_only: bool,
    open_browser: bool,
    client_id: Option<String>,
    wait_secs: u64,
}

fn parse_args() -> Result<Args> {
    let mut name = None;
    let mut url = None;
    let mut discover_only = false;
    let mut open_browser = true;
    let mut client_id = None;
    let mut wait_secs = 300;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => name = args.next(),
            "--url" => url = args.next(),
            "--client-id" => client_id = args.next(),
            "--wait-secs" => {
                wait_secs = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .context("--wait-secs needs a number")?
            }
            "--discover-only" => discover_only = true,
            "--no-open" => open_browser = false,
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(Args {
        name: name.context("--name is required")?,
        url: url.context("--url is required")?,
        discover_only,
        open_browser,
        client_id,
        wait_secs,
    })
}

/// Create the connector entry, or switch an existing one to OAuth. An existing
/// entry keeps its `enabled` flag, and `update_server` keeps every other field.
fn save_connector(name: &str, url: &str, account: &str) -> Result<()> {
    let existing = list_servers()?.into_iter().find(|s| s.name == name);
    let spec = McpServerSpec {
        name: name.to_string(),
        command: String::new(),
        args: Vec::new(),
        enabled: existing.as_ref().is_none_or(|s| s.enabled),
        endpoint: Some(url.to_string()),
        auth_ref: Some(account.to_string()),
        auth_type: Some("oauth".to_string()),
    };
    if existing.is_some() {
        update_server(name, &spec)
    } else {
        add_server(&spec)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let account = credential_account(&args.name);

    if args.discover_only {
        let server_url = reqwest::Url::parse(&args.url)?;
        let client = http_client()?;
        let challenge: BearerChallenge = probe_challenge(&client, &server_url).await?;
        println!("challenge: {challenge:#?}");
        let found = discover(&client, &server_url, &challenge).await?;
        println!("discovery: {found:#?}");
        return Ok(());
    }

    let pending = begin_login(
        &args.url,
        LoginOptions {
            client_id: args.client_id.clone(),
            callback_timeout: Duration::from_secs(args.wait_secs),
            ..LoginOptions::default()
        },
    )
    .await?;
    println!("redirect_uri: {}", pending.redirect_uri);
    println!("authorization_url: {}", pending.authorization_url);
    if args.open_browser {
        let status = Command::new("open")
            .arg(&pending.authorization_url)
            .status()?;
        if !status.success() {
            bail!("`open` exited with {status}");
        }
        println!(
            "browser opened; waiting up to {}s for sign-in…",
            args.wait_secs
        );
    } else {
        println!(
            "open the URL above; waiting up to {}s for sign-in…",
            args.wait_secs
        );
    }

    let credential = pending.complete(&account).await?;
    save_connector(&args.name, &args.url, &account)?;
    println!(
        "signed in: credential stored under {account}, expires_at={:?}, refresh={}",
        credential.expires_at,
        credential.refresh_token.is_some()
    );
    Ok(())
}
