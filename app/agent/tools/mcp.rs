use std::path::Path;
use std::thread;

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use codescribe_core::mcp::{McpClient, McpConfigFile, McpServerConfig, McpTool};
use tracing::{info, warn};

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
        let servers = config
            .enabled_servers()
            .map(|(name, server_config)| (name.to_string(), server_config.clone()))
            .collect::<Vec<_>>();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to create MCP discovery runtime")?;

        runtime.block_on(async move {
            let mut discovered = Vec::new();
            for (server_name, server_config) in servers {
                let client = McpClient::new(server_config.clone());
                match client.list_tools().await {
                    Ok(tools) => {
                        for tool in tools {
                            discovered.push(DiscoveredMcpTool {
                                server_name: server_name.clone(),
                                server_config: server_config.clone(),
                                tool,
                            });
                        }
                    }
                    Err(error) => {
                        warn!("MCP server '{server_name}' discovery failed: {error}");
                    }
                }
            }
            Ok(discovered)
        })
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

    use super::{public_tool_name, register_mcp_tools_from_config_path};

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
