//! MCP (Model Context Protocol) client surface — external tool servers.
//!
//! Three concerns, deliberately separated:
//!
//! - [`client`] speaks the protocol: handshake, tool discovery, and the
//!   `mcp.json` config file shape.
//! - [`config_store`] owns durable server CRUD plus the blocking probe/test calls
//!   the Settings UI drives.
//! - [`secret_migration`] moves plaintext `env` secrets out of `mcp.json` and
//!   resolves a server's environment at spawn time.
//!
//! MCP is optional throughout: a missing `mcp.json` is a normal state, not an
//! error, and callers degrade to native tools only.

pub mod client;
/// On-disk MCP server config load/save and secret-aware field handling.
pub mod config_store;
/// One-shot migration of legacy MCP secrets into the current secret store.
pub mod secret_migration;

pub use client::{
    McpClient, McpConfigFile, McpHandshake, McpProbe, McpServerConfig, McpServerInfo, McpTool,
    default_mcp_config_path,
};
pub use config_store::{
    McpProbeSummary, McpServerSpec, McpServerSummary, add_server, list_servers,
    probe_server_blocking, remove_server, test_server_blocking, update_server,
};
pub use secret_migration::{
    SecretMigrationReport, format_report as format_secret_migration_report,
    migrate_plaintext_env_secrets, resolve_server_env,
};
