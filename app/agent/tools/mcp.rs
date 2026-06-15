use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread;

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use codescribe_core::mcp::{McpClient, McpConfigFile, McpServerConfig, McpTool};
use tracing::{info, warn};

/// Per-server runtime discovery outcome captured during `register` (real spawn
/// + `tools/list` handshake). Read back by the Settings Engine tab so the UI
///   reflects what actually happened instead of guessing.
#[derive(Debug, Clone)]
enum ServerRuntime {
    /// Server responded to `tools/list`; payload is the exposed tool count.
    Tools(usize),
    /// Server is configured + enabled but discovery failed; payload is the
    /// concrete reason (spawn failure, command not found, parse error, …).
    Failed(String),
    /// Server is present in config but disabled (`"enabled": false`).
    Disabled,
}

/// Cache of the last runtime discovery, keyed by server name. Written once per
/// agent-runtime init, read on demand by the read-only Engine settings tab.
static MCP_RUNTIME: OnceLock<Mutex<BTreeMap<String, ServerRuntime>>> = OnceLock::new();

fn runtime_cache() -> &'static Mutex<BTreeMap<String, ServerRuntime>> {
    MCP_RUNTIME.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn record_runtime(snapshot: BTreeMap<String, ServerRuntime>) {
    let mut guard = runtime_cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *guard = snapshot;
}

fn anyhow_root_cause(error: &anyhow::Error) -> String {
    error.root_cause().to_string()
}

/// Visual tone for an MCP status row, mapped to concrete `ui_colors` by the
/// Settings layer. Keeping this UI-agnostic avoids coupling agent tooling to
/// AppKit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRowTone {
    Good,
    Warn,
    Bad,
    Neutral,
}

/// One labelled status line in the Engine tab's "MCP Servers" section.
pub struct McpStatusRow {
    pub label: String,
    pub value: String,
    pub tone: McpRowTone,
}

/// Honest snapshot of MCP config + runtime state for the Settings UI.
pub struct McpStatusReport {
    pub config_path_display: String,
    rows: Vec<McpStatusRow>,
}

impl McpStatusReport {
    pub fn summary_rows(&self) -> &[McpStatusRow] {
        &self.rows
    }

    fn single(config_path_display: String, label: &str, value: String, tone: McpRowTone) -> Self {
        Self {
            config_path_display,
            rows: vec![McpStatusRow {
                label: label.to_string(),
                value,
                tone,
            }],
        }
    }
}

/// Probe MCP config + cached runtime discovery for the read-only Engine tab.
///
/// Cheap: reads/parses `mcp.json` (no server spawning) and merges in whatever
/// the last real discovery recorded. Never claims "MCP doesn't exist" when the
/// config file is present — a present-but-broken config reports the concrete
/// failure instead.
pub fn probe_mcp_status() -> McpStatusReport {
    let path = match codescribe_core::mcp::default_mcp_config_path() {
        Ok(path) => path,
        Err(error) => {
            return McpStatusReport::single(
                "unavailable".to_string(),
                "Status:",
                format!("config path unavailable: {error}"),
                McpRowTone::Bad,
            );
        }
    };
    probe_mcp_status_at(&path)
}

fn probe_mcp_status_at(path: &Path) -> McpStatusReport {
    let config_path_display = path.display().to_string();

    if !path.exists() {
        return McpStatusReport::single(
            config_path_display,
            "Status:",
            "no mcp.json (optional — MCP off)".to_string(),
            McpRowTone::Neutral,
        );
    }

    let config = match McpConfigFile::load(path) {
        Ok(config) => config,
        Err(error) => {
            return McpStatusReport::single(
                config_path_display,
                "Config error:",
                anyhow_root_cause(&error),
                McpRowTone::Bad,
            );
        }
    };

    if config.servers.is_empty() {
        return McpStatusReport::single(
            config_path_display,
            "Status:",
            "config present, no servers defined".to_string(),
            McpRowTone::Warn,
        );
    }

    let runtime = runtime_cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();

    let mut names: Vec<&String> = config.servers.keys().collect();
    names.sort();
    let mut rows = Vec::with_capacity(names.len());
    for name in names {
        let enabled = config
            .servers
            .get(name)
            .and_then(|server| server.enabled)
            .unwrap_or(true);
        let (value, tone) = match runtime.get(name) {
            Some(ServerRuntime::Tools(count)) => (format!("{count} tool(s)"), McpRowTone::Good),
            Some(ServerRuntime::Failed(reason)) => (format!("failed: {reason}"), McpRowTone::Bad),
            Some(ServerRuntime::Disabled) => ("disabled".to_string(), McpRowTone::Neutral),
            None if !enabled => ("disabled".to_string(), McpRowTone::Neutral),
            None => (
                "configured (agent not started)".to_string(),
                McpRowTone::Warn,
            ),
        };
        rows.push(McpStatusRow {
            label: format!("{name}:"),
            value,
            tone,
        });
    }

    McpStatusReport {
        config_path_display,
        rows,
    }
}

pub fn register(registry: &mut ToolRegistry) {
    let path = match codescribe_core::mcp::default_mcp_config_path() {
        Ok(path) => path,
        Err(error) => {
            warn!("MCP config path unavailable: {error}");
            return;
        }
    };

    match register_mcp_tools_from_config_path(registry, &path) {
        Ok(count) if count > 0 => {
            info!("Registered {count} MCP tool(s) from {}", path.display());
        }
        Ok(_) => {}
        Err(error) => {
            warn!("MCP tool registration skipped: {error}");
        }
    }
}

pub(crate) fn register_mcp_tools_from_config_path(
    registry: &mut ToolRegistry,
    path: &Path,
) -> Result<usize> {
    let Some(config) = McpConfigFile::load_optional(path)? else {
        return Ok(0);
    };
    register_mcp_tools_from_config(registry, config)
}

fn register_mcp_tools_from_config(
    registry: &mut ToolRegistry,
    config: McpConfigFile,
) -> Result<usize> {
    let discovered = discover_mcp_tools_blocking(config)?;
    let mut registered = 0usize;

    for discovered_tool in discovered {
        let public_name =
            match public_tool_name(&discovered_tool.server_name, &discovered_tool.tool.name) {
                Ok(name) => name,
                Err(error) => {
                    warn!("Skipping MCP tool with invalid name: {error}");
                    continue;
                }
            };

        let original_tool_name = discovered_tool.tool.name.clone();
        let server_name = discovered_tool.server_name.clone();
        let client_config = discovered_tool.server_config.clone();
        let description = discovered_tool.tool.description.clone().unwrap_or_else(|| {
            format!("MCP tool '{original_tool_name}' from server '{server_name}'")
        });

        let definition = ToolDefinition {
            name: public_name,
            description,
            input_schema: discovered_tool.tool.input_schema.clone(),
        };

        let register_result = registry.register(
            definition,
            Box::new(move |input| {
                let client = McpClient::new(client_config.clone());
                let tool_name = original_tool_name.clone();
                let server = server_name.clone();
                Box::pin(async move {
                    match client.call_tool(&tool_name, input).await {
                        Ok(output) => output,
                        Err(error) => vec![ToolResultContent::Error(format!(
                            "MCP tool '{server}/{tool_name}' failed: {error}"
                        ))],
                    }
                })
            }),
        );

        if let Err(error) = register_result {
            warn!("Skipping duplicate MCP tool registration: {error}");
            continue;
        }

        registered += 1;
    }

    Ok(registered)
}

#[derive(Debug)]
struct DiscoveredMcpTool {
    server_name: String,
    server_config: McpServerConfig,
    tool: McpTool,
}

fn discover_mcp_tools_blocking(config: McpConfigFile) -> Result<Vec<DiscoveredMcpTool>> {
    thread::spawn(move || -> Result<Vec<DiscoveredMcpTool>> {
        // Capture EVERY configured server (enabled or not) so the runtime cache
        // reports disabled servers truthfully instead of as "missing".
        let mut servers: Vec<(String, McpServerConfig, bool)> = config
            .servers
            .iter()
            .map(|(name, server_config)| {
                let enabled = server_config.enabled.unwrap_or(true);
                (name.clone(), server_config.clone(), enabled)
            })
            .collect();
        servers.sort_by(|a, b| a.0.cmp(&b.0));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to create MCP discovery runtime")?;

        let (discovered, status) = runtime.block_on(async move {
            let mut discovered = Vec::new();
            let mut status: BTreeMap<String, ServerRuntime> = BTreeMap::new();
            for (server_name, server_config, enabled) in servers {
                if !enabled {
                    status.insert(server_name, ServerRuntime::Disabled);
                    continue;
                }
                let client = McpClient::new(server_config.clone());
                match client.list_tools().await {
                    Ok(tools) => {
                        status.insert(server_name.clone(), ServerRuntime::Tools(tools.len()));
                        for tool in tools {
                            discovered.push(DiscoveredMcpTool {
                                server_name: server_name.clone(),
                                server_config: server_config.clone(),
                                tool,
                            });
                        }
                    }
                    Err(error) => {
                        // Concrete root cause (spawn failure, command not found,
                        // parse error, timeout, …) — surfaced to logs AND the UI.
                        let reason = anyhow_root_cause(&error);
                        warn!("MCP server '{server_name}' discovery failed: {reason}");
                        status.insert(server_name, ServerRuntime::Failed(reason));
                    }
                }
            }
            (discovered, status)
        });

        record_runtime(status);
        Ok(discovered)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("MCP discovery thread panicked"))?
}

fn public_tool_name(server_name: &str, tool_name: &str) -> Result<String> {
    validate_name_part("server", server_name)?;
    validate_name_part("tool", tool_name)?;
    Ok(format!("mcp__{server_name}__{tool_name}"))
}

fn validate_name_part(kind: &str, name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("MCP {kind} name is empty");
    }

    if name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Ok(());
    }

    bail!("MCP {kind} name '{name}' contains unsupported characters")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use codescribe_core::agent::{ToolRegistry, ToolResultContent};
    use serde_json::json;

    use super::{
        McpRowTone, probe_mcp_status_at, public_tool_name, register_mcp_tools_from_config_path,
    };

    #[test]
    fn probe_reports_missing_config_as_optional_not_broken() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // never created
        let report = probe_mcp_status_at(&path);
        let rows = report.summary_rows();
        assert_eq!(rows.len(), 1);
        // Honest: "no mcp.json" is neutral/optional, NOT a hard error.
        assert_eq!(rows[0].tone, McpRowTone::Neutral);
        assert!(
            rows[0].value.contains("no mcp.json"),
            "got: {}",
            rows[0].value
        );
    }

    #[test]
    fn probe_reports_parse_error_with_reason_when_config_present() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        fs::write(&path, "{ this is not json").expect("write garbage config");
        let report = probe_mcp_status_at(&path);
        let rows = report.summary_rows();
        // Config IS present but broken — must surface a concrete error, never
        // claim "MCP doesn't exist".
        assert_eq!(rows[0].tone, McpRowTone::Bad);
        assert_eq!(rows[0].label, "Config error:");
        assert!(!rows[0].value.is_empty());
    }

    #[test]
    fn probe_reports_present_config_with_no_servers() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        fs::write(&path, json!({ "mcpServers": {} }).to_string()).expect("write config");
        let report = probe_mcp_status_at(&path);
        let rows = report.summary_rows();
        assert_eq!(rows[0].tone, McpRowTone::Warn);
        assert!(rows[0].value.contains("no servers"));
    }

    #[test]
    fn probe_lists_configured_server_before_runtime_discovery() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        // Unique name that the runtime cache cannot already hold.
        let config = json!({
            "mcpServers": {
                "probe_only_unprobed_server": {
                    "command": "python3",
                    "args": ["x.py"],
                    "enabled": true
                }
            }
        });
        fs::write(&path, config.to_string()).expect("write config");
        let report = probe_mcp_status_at(&path);
        let row = report
            .summary_rows()
            .iter()
            .find(|r| r.label == "probe_only_unprobed_server:")
            .expect("server row present");
        assert_eq!(row.tone, McpRowTone::Warn);
        assert!(
            row.value.contains("agent not started"),
            "got: {}",
            row.value
        );
    }

    #[tokio::test]
    async fn registers_and_dispatches_mcp_tool_from_config() {
        let temp = tempfile::tempdir().expect("temp dir should be created");
        let config_path = temp.path().join("mcp.json");
        let script = repo_root()
            .join("tests")
            .join("fixtures")
            .join("mock_mcp.py");
        let config = json!({
            "mcpServers": {
                "mock": {
                    "command": "python3",
                    "args": [script],
                    "enabled": true,
                    "timeout_seconds": 5
                }
            }
        });
        fs::write(
            &config_path,
            serde_json::to_string(&config).expect("config should serialize"),
        )
        .expect("config should be written");

        let mut registry = ToolRegistry::new();
        let registered = register_mcp_tools_from_config_path(&mut registry, &config_path)
            .expect("MCP config should register");

        assert_eq!(registered, 1);
        let names = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["mcp__mock__echo".to_string()]);

        let output = registry
            .dispatch("mcp__mock__echo", json!({ "message": "from app" }))
            .await
            .expect("MCP dispatch should complete");

        assert_eq!(
            output,
            vec![ToolResultContent::Text("echo: from app".to_string())]
        );
    }

    #[test]
    fn rejects_unsafe_public_tool_name_parts() {
        let error = public_tool_name("bad server", "echo")
            .expect_err("server names with spaces should be rejected");
        assert!(
            error.to_string().contains("unsupported characters"),
            "unexpected error: {error}"
        );
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }
}
