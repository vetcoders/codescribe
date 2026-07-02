//! MCP management surface — read/write UniFFI wrapper over the core MCP config
//! store (`core/mcp/config_store.rs`). Distinct from the read-only
//! `agent_status` slice: this one MUTATES `~/.codescribe/mcp.json` (add / update
//! / remove) and can spawn a server to test it.
//!
//! Every mutation goes through the core store's atomic, unknown-field-preserving
//! writer, so a hand-edited config is never clobbered. Sync-only: the CRUD calls
//! are cheap disk I/O; `test_server` blocks on a one-shot discovery handshake
//! (bounded by a 10s timeout) run on the core's dedicated thread + runtime.

use std::time::Duration;

use codescribe_core::mcp::{
    McpServerSpec, McpServerSummary, add_server, list_servers, remove_server, test_server_blocking,
    update_server,
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
}

impl From<McpServerSummary> for CsMcpServer {
    fn from(summary: McpServerSummary) -> Self {
        Self {
            name: summary.name,
            command: summary.command,
            args: summary.args,
            env_keys: summary.env_keys,
            enabled: summary.enabled,
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
}

impl From<&CsMcpServerInput> for McpServerSpec {
    fn from(input: &CsMcpServerInput) -> Self {
        Self {
            name: input.name.clone(),
            command: input.command.clone(),
            args: input.args.clone(),
            enabled: input.enabled,
        }
    }
}

/// Result of testing one server: whether the handshake succeeded, the live tool
/// count, and (on failure) a concrete reason. Never throws — a failed test is a
/// normal, displayable outcome, not an FFI error.
#[derive(uniffi::Record)]
pub struct CsMcpTestResult {
    pub ok: bool,
    pub tool_count: u32,
    pub error: String,
}

/// Read/write handle over the MCP config store. Stateless: every call re-reads
/// on-disk truth so Swift always sees current state.
#[derive(uniffi::Object, Default)]
pub struct CodescribeMcpAdmin {}

#[uniffi::export]
impl CodescribeMcpAdmin {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    /// List configured servers (sorted). A missing `mcp.json` is an empty list.
    pub fn list_servers(&self) -> Result<Vec<CsMcpServer>, CsError> {
        let servers = list_servers().map_err(config_err)?;
        Ok(servers.into_iter().map(CsMcpServer::from).collect())
    }

    /// Add a new server. Errors if the name already exists or is invalid.
    pub fn add_server(&self, server: CsMcpServerInput) -> Result<(), CsError> {
        add_server(&McpServerSpec::from(&server)).map_err(config_err)
    }

    /// Update the named server's spawn shape, preserving its env + unknown fields.
    pub fn update_server(&self, name: String, server: CsMcpServerInput) -> Result<(), CsError> {
        update_server(&name, &McpServerSpec::from(&server)).map_err(config_err)
    }

    /// Remove the named server. Errors if it does not exist.
    pub fn remove_server(&self, name: String) -> Result<(), CsError> {
        remove_server(&name).map_err(config_err)
    }

    /// Spawn the named server and report its live tool count. Bounded by a 10s
    /// timeout. A failed handshake is returned as `ok == false` with a reason,
    /// never as a thrown error.
    pub fn test_server(&self, name: String) -> CsMcpTestResult {
        match test_server_blocking(&name, TEST_TIMEOUT) {
            Ok(count) => CsMcpTestResult {
                ok: true,
                tool_count: count as u32,
                error: String::new(),
            },
            Err(error) => CsMcpTestResult::failure(error.root_cause().to_string()),
        }
    }
}

impl CsMcpTestResult {
    fn failure(error: String) -> Self {
        Self {
            ok: false,
            tool_count: 0,
            error,
        }
    }
}

fn config_err(error: anyhow::Error) -> CsError {
    CsError::Config {
        msg: error.to_string(),
    }
}
