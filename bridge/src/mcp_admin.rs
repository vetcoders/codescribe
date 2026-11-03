//! MCP management surface — read/write UniFFI wrapper over the core MCP config
//! store (`core/mcp/config_store.rs`). Distinct from the read-only
//! `agent_status` slice: this one MUTATES `~/.codescribe/mcp.json` (add / update
//! / remove) and can spawn a server to test it.
//!
//! Also exposes the B2 tool-permission gateway: durable allow/ask/deny policy
//! (settings.json `agent.permissions`) and the capability list from the same
//! tool registry the agent dispatcher uses.
//!
//! Every mutation goes through the core store's atomic, unknown-field-preserving
//! writer, so a hand-edited config is never clobbered. Sync-only: the CRUD calls
//! are cheap disk I/O; `test_server` blocks on a one-shot discovery handshake
//! (bounded by a 10s timeout) run on the core's dedicated thread + runtime, and
//! records the outcome in the app's MCP evidence owner so Agent Diagnostics
//! reads the same test the MCP tab just showed.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use codescribe_core::agent::{AgentPermissions, PermissionLevel, ToolRegistry, tool_grants};
use codescribe_core::config::keychain::{delete_key, save_key};
use codescribe_core::mcp::oauth::{self, LoginOptions};
use codescribe_core::mcp::{
    McpServerSpec, McpServerSummary, add_server, list_servers, remove_server, update_server,
};

use crate::CsError;

/// How long `test_server` waits for the full spawn + `initialize` + `tools/list`
/// handshake before giving up.
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// One configured MCP server for the management list. Carries env var NAMES only
/// — secret values never cross the FFI boundary.
#[derive(uniffi::Record)]
pub struct CsMcpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub enabled: bool,
    pub transport: String,
    pub endpoint: String,
    pub auth_ref: String,
    /// True when the credential behind `auth_ref` is an OAuth grant
    /// (`auth_type: "oauth"` in mcp.json), not a pasted bearer token.
    pub oauth: bool,
}

impl From<McpServerSummary> for CsMcpServer {
    /// Map core McpServerSummary into the UniFFI bridge DTO for Swift admin UI.
    fn from(summary: McpServerSummary) -> Self {
        Self {
            name: summary.name,
            command: summary.command,
            args: summary.args,
            env_keys: summary.env_keys,
            enabled: summary.enabled,
            transport: if summary.endpoint.is_some() {
                "remote".to_string()
            } else {
                "stdio".to_string()
            },
            endpoint: summary.endpoint.unwrap_or_default(),
            auth_ref: summary.auth_ref.unwrap_or_default(),
            oauth: summary.auth_type.as_deref() == Some("oauth"),
        }
    }
}

/// Desired spawn shape from the add / edit form. Env is not edited here (secrets
/// stay file-side); an update preserves any existing `env` block.
#[derive(uniffi::Record)]
pub struct CsMcpServerInput {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    /// Empty for local stdio; http(s) URL for a remote Streamable HTTP server.
    pub endpoint: String,
    /// Existing Keychain account to preserve during update/toggle.
    pub auth_ref: String,
    /// Write-only bearer token. It is saved straight to Keychain and never
    /// serialized into mcp.json or returned by `list_servers`.
    pub token: String,
    /// The connector signs in with OAuth instead of a bearer token. The grant
    /// itself is obtained through `start_mcp_sign_in` / `await_mcp_sign_in`.
    pub oauth: bool,
}

impl From<&CsMcpServerInput> for McpServerSpec {
    /// Map UniFFI CsMcpServerInput into the core MCP config write shape.
    fn from(input: &CsMcpServerInput) -> Self {
        Self {
            name: input.name.clone(),
            command: input.command.clone(),
            args: input.args.clone(),
            enabled: input.enabled,
            endpoint: (!input.endpoint.trim().is_empty()).then(|| input.endpoint.clone()),
            auth_ref: (!input.auth_ref.trim().is_empty()).then(|| input.auth_ref.clone()),
            // `Some("oauth")` marks the row; `None` leaves an existing
            // `auth_type` untouched on update, and the explicit OAuth-off
            // transition is handled in `update_server`.
            auth_type: input.oauth.then(|| "oauth".to_string()),
        }
    }
}

/// Result of testing one server: whether the handshake succeeded, the identity
/// it advertised in the `initialize` handshake (server name / version / protocol
/// — empty when the server omits them), the live tool count, and (on failure) a
/// concrete reason. Never throws — a failed test is a normal, displayable
/// outcome, not an FFI error.
#[derive(uniffi::Record)]
pub struct CsMcpTestResult {
    pub ok: bool,
    pub tool_count: u32,
    pub server_name: String,
    pub server_version: String,
    pub protocol_version: String,
    pub error: String,
}

/// One step of the MCP OAuth sign-in flow — the shape of
/// `CsAccountLoginResult` for provider accounts, minus Rust-composed prose:
/// `status` is a wire value Swift turns into a sentence it owns
/// (LOCALIZATION_LEDGER.md §4), and `detail` carries only propagated content —
/// an upstream error chain, or the superseding server's name. `auth_url` is
/// present after `start_mcp_sign_in` when the UI should open a browser;
/// `signed_in` flips once the exchange completed and the credential is in the
/// Keychain.
#[derive(uniffi::Record)]
pub struct CsMcpSignInResult {
    pub server: String,
    /// `started` | `signed_in` | `failed` | `timeout` | `superseded` | `idle`.
    pub status: String,
    /// Upstream content for `failed` (error chain) and `superseded` (the other
    /// server's name); empty otherwise. Never a sentence composed in Rust.
    pub detail: String,
    pub auth_url: Option<String>,
    pub signed_in: bool,
}

/// The one in-flight OAuth attempt. A second `start_mcp_sign_in` supersedes it
/// (dropping the pending login closes its loopback listener), so fixed ports
/// and browser tabs never race two logins — same policy as provider accounts.
struct PendingMcpSignIn {
    server: String,
    login: oauth::PendingLogin,
}

fn active_mcp_sign_in() -> &'static Mutex<Option<PendingMcpSignIn>> {
    static STATE: OnceLock<Mutex<Option<PendingMcpSignIn>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

/// Read/write handle over the MCP config store. Stateless: every call re-reads
/// on-disk truth so Swift always sees current state.
/// One persisted "always allow" grant: the `server:tool` key the approval gate
/// checks, plus when the operator granted it (RFC 3339).
#[derive(uniffi::Record)]
pub struct CsToolGrant {
    pub key: String,
    pub granted_at: String,
}

/// One tool capability from the live registry + effective permission level.
#[derive(uniffi::Record)]
pub struct CsToolCapability {
    pub name: String,
    /// Canonical identity (`server:upstream` or `native:name`).
    pub identity: String,
    pub origin: String,
    pub server: String,
    pub risk: String,
    /// Effective level the gate applies to the next call: `allow` | `ask` | `deny`.
    pub effective: String,
    /// Rule behind `effective`: `tool` (individual), `server` or `default`
    /// (inherited), `thread` (session override).
    pub rule_source: String,
    pub requires_approval_flag: bool,
}

/// Durable permission policy snapshot for Settings.
#[derive(uniffi::Record)]
pub struct CsPermissionPolicy {
    pub default_level: String,
    pub read_only_default: String,
    pub side_effect_default: String,
    /// `identity=level` pairs (e.g. `desktop-commander:write_file=allow`).
    pub tools: Vec<String>,
    /// `server=level` pairs.
    pub servers: Vec<String>,
}

/// UniFFI handle exposing MCP server CRUD plus the B2 permission surface.
/// Stateless: every call re-reads on-disk truth, so Swift never caches a
/// snapshot that a hand edit or another process could have invalidated.
#[derive(uniffi::Object, Default)]
pub struct CodescribeMcpAdmin {}

#[uniffi::export]
impl CodescribeMcpAdmin {
    /// Construct the handle, initializing logging on first use.
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self::default()
    }

    /// List configured servers (sorted). A missing `mcp.json` is an empty list.
    pub fn list_servers(&self) -> Result<Vec<CsMcpServer>, CsError> {
        let servers = list_servers().map_err(config_err)?;
        Ok(servers.into_iter().map(CsMcpServer::from).collect())
    }

    /// Add a new server. Errors if the name already exists or is invalid.
    pub fn add_server(&self, server: CsMcpServerInput) -> Result<(), CsError> {
        validate_auth_shape(&server)?;
        let mut spec = McpServerSpec::from(&server);
        let stored_account = store_connector_token(&server.name, &server.token)?;
        if stored_account.is_some() {
            spec.auth_ref = stored_account.clone();
        }
        if let Err(error) = add_server(&spec) {
            if let Some(account) = stored_account {
                let _ = delete_key(&account);
            }
            return Err(config_err(error));
        }
        // Remote connectors enter B2 through the same server policy registry.
        // Pin the server default to Ask only when the operator has not already
        // chosen an explicit setting.
        if spec.endpoint.is_some() {
            let policy = AgentPermissions::load();
            if !policy.servers.contains_key(&server.name)
                && let Err(error) =
                    AgentPermissions::set_server_level(&server.name, PermissionLevel::Ask)
            {
                let _ = remove_server(&server.name);
                if let Some(account) = stored_account {
                    let _ = delete_key(&account);
                }
                return Err(config_err(error));
            }
        }
        Ok(())
    }

    /// Update the named server's spawn shape, preserving its env + unknown fields.
    /// Turning OAuth off clears the grant: the credential leaves the Keychain
    /// and `auth_type`/`auth_ref` leave mcp.json before any new bearer lands.
    pub fn update_server(&self, name: String, server: CsMcpServerInput) -> Result<(), CsError> {
        validate_auth_shape(&server)?;
        let was_oauth = list_servers()
            .map_err(config_err)?
            .into_iter()
            .find(|entry| entry.name == name)
            .is_some_and(|entry| entry.auth_type.as_deref() == Some("oauth"));
        let mut spec = McpServerSpec::from(&server);
        if let Some(account) = store_connector_token(&server.name, &server.token)? {
            spec.auth_ref = Some(account);
        }
        if was_oauth && !server.oauth {
            // The form passes the row's old auth_ref through; for an OAuth row
            // that is the credential account being signed out, not a bearer.
            let credential = oauth::credential_account(&name);
            if spec.auth_ref.as_deref() == Some(credential.as_str()) {
                spec.auth_ref = None;
            }
            let mut cleared = spec.clone();
            cleared.auth_ref = None;
            cleared.auth_type = None;
            update_server(&name, &cleared).map_err(config_err)?;
            let _ = delete_key(&credential);
            if spec.auth_ref.is_some() {
                update_server(&name, &spec).map_err(config_err)?;
            }
            return Ok(());
        }
        update_server(&name, &spec).map_err(config_err)
    }

    /// Remove the named server. Errors if it does not exist.
    pub fn remove_server(&self, name: String) -> Result<(), CsError> {
        let auth_ref = list_servers()
            .map_err(config_err)?
            .into_iter()
            .find(|server| server.name == name)
            .and_then(|server| server.auth_ref);
        remove_server(&name).map_err(config_err)?;
        if let Some(account) = auth_ref {
            delete_key(&account).map_err(config_err)?;
        }
        Ok(())
    }

    /// List persisted "always allow" tool grants, newest write last. Each key is
    /// `"<server>:<upstream_tool>"` — the same identity the approval gate checks.
    pub fn list_tool_grants(&self) -> Result<Vec<CsToolGrant>, CsError> {
        let grants = tool_grants::list().map_err(config_err)?;
        Ok(grants
            .into_iter()
            .map(|(key, granted_at)| CsToolGrant { key, granted_at })
            .collect())
    }

    /// Revoke one grant so the tool asks again on its next call. Revoking a key
    /// that is not granted succeeds — the caller's intent (this tool must ask)
    /// already holds. Also clears the matching settings.json tool preference.
    pub fn revoke_tool_grant(&self, key: String) -> Result<(), CsError> {
        tool_grants::revoke(&key).map_err(config_err)?;
        // Align settings.json so a revoked grant cannot re-allow via policy.
        let mut policy = AgentPermissions::load();
        if policy.tools.remove(&key).is_some() {
            policy.save().map_err(config_err)?;
        }
        Ok(())
    }

    /// Snapshot of durable `agent.permissions` (settings.json).
    pub fn get_permission_policy(&self) -> CsPermissionPolicy {
        let p = AgentPermissions::load();
        CsPermissionPolicy {
            default_level: p.default.as_str().to_string(),
            read_only_default: p.read_only_default.as_str().to_string(),
            side_effect_default: p.side_effect_default.as_str().to_string(),
            tools: p
                .tools
                .iter()
                .map(|(k, v)| format!("{}={}", k, v.as_str()))
                .collect(),
            servers: p
                .servers
                .iter()
                .map(|(k, v)| format!("{}={}", k, v.as_str()))
                .collect(),
        }
    }

    /// Set global risk-class defaults (`allow` | `ask` | `deny`).
    pub fn set_permission_defaults(
        &self,
        default_level: String,
        read_only_default: String,
        side_effect_default: String,
    ) -> Result<(), CsError> {
        let default = PermissionLevel::parse(&default_level).map_err(config_err)?;
        let read_only = PermissionLevel::parse(&read_only_default).map_err(config_err)?;
        let side_effect = PermissionLevel::parse(&side_effect_default).map_err(config_err)?;
        AgentPermissions::set_defaults(default, read_only, side_effect).map_err(config_err)
    }

    /// Set durable per-tool preference. Identity is `server:tool` or `native:name`.
    pub fn set_tool_permission(&self, identity: String, level: String) -> Result<(), CsError> {
        let level = PermissionLevel::parse(&level).map_err(config_err)?;
        AgentPermissions::set_tool_level(&identity, level).map_err(config_err)
    }

    /// Remove the durable per-tool rule so the tool inherits again.
    pub fn clear_tool_permission(&self, identity: String) -> Result<(), CsError> {
        AgentPermissions::clear_tool_level(&identity).map_err(config_err)
    }

    /// Set durable per-MCP-server preference.
    pub fn set_server_permission(&self, server: String, level: String) -> Result<(), CsError> {
        let level = PermissionLevel::parse(&level).map_err(config_err)?;
        AgentPermissions::set_server_level(&server, level).map_err(config_err)
    }

    /// Live capability list from the same registry the agent dispatcher builds.
    /// Unregistered tools cannot appear.
    pub fn list_tool_capabilities(&self) -> Vec<CsToolCapability> {
        let mut registry = ToolRegistry::new();
        codescribe::agent::tools::register_all_tools(&mut registry);
        registry
            .set_policy(AgentPermissions::load().with_legacy_grants(tool_grants::load_granted()));
        registry
            .capabilities()
            .into_iter()
            .map(|c| CsToolCapability {
                name: c.name,
                identity: c.identity,
                origin: c.origin,
                server: c.server.unwrap_or_default(),
                risk: c.risk,
                effective: c.effective.as_str().to_string(),
                rule_source: c.source.as_str().to_string(),
                requires_approval_flag: c.requires_approval_flag,
            })
            .collect()
    }

    /// Begin OAuth sign-in for a remote server: probe the 401 challenge,
    /// discover the authorization server, register a client and bind the
    /// loopback listener — everything short of the human. Returns the
    /// authorize URL for Swift to open; `await_mcp_sign_in` then blocks on
    /// the browser redirect. Nothing is stored until that completes.
    pub async fn start_mcp_sign_in(
        &self,
        name: String,
        endpoint: String,
    ) -> Result<CsMcpSignInResult, CsError> {
        if endpoint.trim().is_empty() {
            return Err(CsError::Config {
                msg: "OAuth sign-in needs a remote server URL".to_string(),
            });
        }
        {
            let mut guard = mcp_sign_in_lock()?;
            // Dropping a pending login closes its loopback listener.
            guard.take();
        }
        let login = crate::application_runtime::run(async move {
            oauth::begin_login(&endpoint, LoginOptions::default()).await
        })
        .await?
        .map_err(config_err)?;
        let auth_url = login.authorization_url.clone();
        let mut guard = mcp_sign_in_lock()?;
        *guard = Some(PendingMcpSignIn {
            server: name.clone(),
            login,
        });
        Ok(CsMcpSignInResult {
            server: name,
            status: "started".to_string(),
            detail: String::new(),
            auth_url: Some(auth_url),
            signed_in: false,
        })
    }

    /// Block until the in-flight MCP sign-in completes, fails or times out.
    /// Swift calls this right after `start_mcp_sign_in` opened the browser.
    /// On success the credential is already in the Keychain and the server's
    /// mcp.json row flips to `auth_type: "oauth"`. On timeout the pending
    /// login is dropped, which shuts the loopback listener down.
    pub async fn await_mcp_sign_in(
        &self,
        name: String,
        timeout_seconds: u64,
    ) -> Result<CsMcpSignInResult, CsError> {
        let pending = { mcp_sign_in_lock()?.take() };
        let Some(pending) = pending else {
            return Ok(mcp_sign_in_result(&name, "idle", ""));
        };
        if pending.server != name {
            return Ok(mcp_sign_in_result(&name, "superseded", &pending.server));
        }
        let account = oauth::credential_account(&name);
        let timeout = Duration::from_secs(timeout_seconds.max(1));
        let outcome = crate::application_runtime::run(async move {
            tokio::time::timeout(timeout, pending.login.complete(&account)).await
        })
        .await?;
        match outcome {
            Ok(Ok(_credential)) => {
                persist_oauth_entry(&name)?;
                Ok(CsMcpSignInResult {
                    server: name.clone(),
                    status: "signed_in".to_string(),
                    detail: String::new(),
                    auth_url: None,
                    signed_in: true,
                })
            }
            Ok(Err(error)) => Ok(mcp_sign_in_result(&name, "failed", &error.to_string())),
            // Dropping the pending login shut the loopback listener down.
            Err(_elapsed) => Ok(mcp_sign_in_result(&name, "timeout", "")),
        }
    }

    /// Cancel any in-flight MCP sign-in (closes the loopback listener).
    pub fn cancel_mcp_sign_in(&self) {
        if let Ok(mut guard) = mcp_sign_in_lock() {
            guard.take();
        }
    }

    /// Spawn the named server, run the `initialize` + `tools/list` handshake, and
    /// report its advertised identity + live tool count. Bounded by a 10s timeout.
    /// A failed handshake is returned as `ok == false` with a reason, never as a
    /// thrown error. Either outcome becomes connection evidence for that exact
    /// config entry; it never registers tools in the running agent.
    pub fn test_server(&self, name: String) -> CsMcpTestResult {
        match codescribe::agent::tools::mcp::test_configured_server(&name, TEST_TIMEOUT) {
            Ok(summary) => CsMcpTestResult {
                ok: true,
                tool_count: summary.tool_count as u32,
                server_name: summary.server_name.unwrap_or_default(),
                server_version: summary.server_version.unwrap_or_default(),
                protocol_version: summary.protocol_version.unwrap_or_default(),
                error: String::new(),
            },
            Err(error) => CsMcpTestResult::failure(error.root_cause().to_string()),
        }
    }
}

impl CsMcpTestResult {
    /// A failed handshake as a displayable result rather than a thrown error.
    fn failure(error: String) -> Self {
        Self {
            ok: false,
            tool_count: 0,
            server_name: String::new(),
            server_version: String::new(),
            protocol_version: String::new(),
            error,
        }
    }
}

/// Flatten a core store error into the FFI `CsError::Config` variant.
fn config_err(error: anyhow::Error) -> CsError {
    CsError::Config {
        msg: error.to_string(),
    }
}

/// OAuth and a pasted bearer are mutually exclusive on one row, and remote
/// authentication shapes make no sense on a local stdio server.
fn validate_auth_shape(server: &CsMcpServerInput) -> Result<(), CsError> {
    if server.oauth && !server.token.trim().is_empty() {
        return Err(CsError::Config {
            msg: "OAuth and a bearer token cannot be combined".to_string(),
        });
    }
    if server.endpoint.trim().is_empty() && (server.oauth || !server.token.trim().is_empty()) {
        return Err(CsError::Config {
            msg: "Local MCP servers cannot use remote authentication".to_string(),
        });
    }
    Ok(())
}

fn mcp_sign_in_lock() -> Result<std::sync::MutexGuard<'static, Option<PendingMcpSignIn>>, CsError> {
    active_mcp_sign_in().lock().map_err(|_| CsError::Config {
        msg: "MCP sign-in state lock poisoned".to_string(),
    })
}

/// A displayable sign-in outcome that is not `signed_in`.
fn mcp_sign_in_result(server: &str, status: &str, detail: &str) -> CsMcpSignInResult {
    CsMcpSignInResult {
        server: server.to_string(),
        status: status.to_string(),
        detail: detail.to_string(),
        auth_url: None,
        signed_in: false,
    }
}

/// After a completed exchange, point the server's mcp.json row at the stored
/// grant: `auth_ref` = the Keychain credential account, `auth_type: "oauth"`.
fn persist_oauth_entry(name: &str) -> Result<(), CsError> {
    let summary = list_servers()
        .map_err(config_err)?
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| CsError::Config {
            msg: format!("MCP server '{name}' is no longer configured"),
        })?;
    let spec = McpServerSpec {
        name: summary.name,
        command: summary.command,
        args: summary.args,
        enabled: summary.enabled,
        endpoint: summary.endpoint,
        auth_ref: Some(oauth::credential_account(name)),
        auth_type: Some("oauth".to_string()),
    };
    update_server(name, &spec).map_err(config_err)
}

/// Persist a bearer token to the Keychain under a name derived from the server
/// (non-alphanumerics folded to `_`), returning the account to store as
/// `auth_ref`. An empty token is `Ok(None)` — nothing written, nothing to
/// reference. The secret itself never reaches `mcp.json`.
fn store_connector_token(name: &str, token: &str) -> Result<Option<String>, CsError> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    let normalized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let account = format!("MCP_CONNECTOR_{normalized}_TOKEN");
    save_key(&account, token).map_err(config_err)?;
    Ok(Some(account))
}
