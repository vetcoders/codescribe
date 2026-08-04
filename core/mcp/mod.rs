pub mod client;
pub mod config_store;
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
