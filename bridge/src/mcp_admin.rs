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
//! (bounded by a 10s timeout) run on the core's dedicated thread + runtime.

use std::time::Duration;

use codescribe_core::agent::{AgentPermissions, PermissionLevel, ToolRegistry, tool_grants};
use codescribe_core::config::keychain::{delete_key, save_key};
use codescribe_core::mcp::{
    McpServerSpec, McpServerSummary, add_server, list_servers, probe_server_blocking,
    remove_server, update_server,
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
    /// Effective level: `allow` | `ask` | `deny`.
    pub effective: String,
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
    pub fn update_server(&self, name: String, server: CsMcpServerInput) -> Result<(), CsError> {
        let mut spec = McpServerSpec::from(&server);
        if let Some(account) = store_connector_token(&server.name, &server.token)? {
            spec.auth_ref = Some(account);
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
                requires_approval_flag: c.requires_approval_flag,
            })
            .collect()
    }

    /// Spawn the named server, run the `initialize` + `tools/list` handshake, and
    /// report its advertised identity + live tool count. Bounded by a 10s timeout.
    /// A failed handshake is returned as `ok == false` with a reason, never as a
    /// thrown error.
    pub fn test_server(&self, name: String) -> CsMcpTestResult {
        match probe_server_blocking(&name, TEST_TIMEOUT) {
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
