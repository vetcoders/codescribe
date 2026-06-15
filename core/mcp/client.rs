use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;
use tracing::debug;

use crate::agent::ToolResultContent;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Deserialize)]
pub struct McpConfigFile {
    #[serde(rename = "mcpServers", default)]
    pub servers: HashMap<String, McpServerConfig>,
}

impl McpConfigFile {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read MCP config {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse MCP config {}", path.display()))
    }

    pub fn load_optional(path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }
        Self::load(path).map(Some)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

impl McpServerConfig {
    fn timeout(&self) -> Duration {
        self.timeout_seconds
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_TIMEOUT)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default = "default_input_schema")]
    pub input_schema: Value,
}

pub fn default_mcp_config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable is not set")?;
    Ok(PathBuf::from(home).join(".codescribe").join("mcp.json"))
}

#[derive(Debug, Clone)]
pub struct McpClient {
    config: McpServerConfig,
    timeout: Duration,
}

impl McpClient {
    pub fn new(config: McpServerConfig) -> Self {
        let timeout = config.timeout();
        Self { config, timeout }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let mut connection = StdioConnection::spawn(&self.config, self.timeout).await?;
        let result = async {
            connection.initialize().await?;
            let response = connection.request("tools/list", json!({})).await?;
            parse_tools_list(response)
        }
        .await;
        let shutdown = connection.shutdown().await;
        if let Err(error) = shutdown {
            debug!("MCP shutdown after tools/list failed: {error}");
        }
        result
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Vec<ToolResultContent>> {
        let mut connection = StdioConnection::spawn(&self.config, self.timeout).await?;
        let result = async {
            connection.initialize().await?;
            let response = connection
                .request(
                    "tools/call",
                    json!({
                        "name": name,
                        "arguments": arguments,
                    }),
                )
                .await?;
            parse_tool_call_result(response)
        }
        .await;
        let shutdown = connection.shutdown().await;
        if let Err(error) = shutdown {
            debug!("MCP shutdown after tools/call failed: {error}");
        }
        result
    }
}

struct StdioConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    response_timeout: Duration,
}

impl StdioConnection {
    async fn spawn(config: &McpServerConfig, response_timeout: Duration) -> Result<Self> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = command.spawn().map_err(|err| {
            // Give the most common failure a concrete, actionable reason instead
            // of a generic spawn error — this string surfaces in the Engine tab.
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("command not found: '{}'", config.command)
            } else {
                anyhow::Error::new(err)
                    .context(format!("Failed to spawn MCP server '{}'", config.command))
            }
        })?;

        let stdin = child
            .stdin
            .take()
            .context("MCP server stdin was not piped")?;
        let stdout = child
            .stdout
            .take()
            .context("MCP server stdout was not piped")?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
            response_timeout,
        })
    }

    async fn initialize(&mut self) -> Result<()> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "codescribe",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .await?;
        self.notification("notifications/initialized", json!({}))
            .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_message(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        loop {
            let line = timeout(self.response_timeout, self.stdout.next_line())
                .await
                .with_context(|| format!("Timed out waiting for MCP response to '{method}'"))?
                .with_context(|| format!("Failed reading MCP response to '{method}'"))?
                .with_context(|| {
                    format!("MCP server closed stdout before responding to '{method}'")
                })?;

            let message: Value = serde_json::from_str(line.trim())
                .with_context(|| format!("Malformed MCP JSON-RPC message: {}", line.trim()))?;

            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }

            if let Some(error) = message.get("error") {
                bail!("MCP request '{method}' failed: {error}");
            }

            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn notification(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_message(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    async fn write_message(&mut self, message: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&message).context("Failed to serialize MCP message")?;
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .context("Failed to write MCP message")?;
        self.stdin
            .flush()
            .await
            .context("Failed to flush MCP message")
    }

    async fn shutdown(mut self) -> Result<()> {
        let _ = timeout(SHUTDOWN_TIMEOUT, self.request("shutdown", json!({}))).await;
        let _ = self
            .notification("notifications/exit", json!({}))
            .await
            .map_err(|error| debug!("MCP exit notification failed: {error}"));
        drop(self.stdin);

        match timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(wait_result) => {
                wait_result.context("Failed waiting for MCP server shutdown")?;
            }
            Err(_) => {
                self.child
                    .kill()
                    .await
                    .context("Failed to kill MCP server after shutdown timeout")?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ToolsListResult {
    #[serde(default)]
    tools: Vec<McpTool>,
}

fn parse_tools_list(value: Value) -> Result<Vec<McpTool>> {
    let result: ToolsListResult =
        serde_json::from_value(value).context("Failed to parse MCP tools/list result")?;
    Ok(result.tools)
}

#[derive(Debug, Deserialize)]
struct ToolCallResult {
    #[serde(default)]
    content: Vec<McpContentBlock>,
    #[serde(rename = "isError", default)]
    is_error: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum McpContentBlock {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(other)]
    Other,
}

fn parse_tool_call_result(value: Value) -> Result<Vec<ToolResultContent>> {
    let result: ToolCallResult =
        serde_json::from_value(value).context("Failed to parse MCP tools/call result")?;

    let mut output = Vec::new();
    for block in result.content {
        match block {
            McpContentBlock::Text { text } if result.is_error => {
                output.push(ToolResultContent::Error(text));
            }
            McpContentBlock::Text { text } => output.push(ToolResultContent::Text(text)),
            McpContentBlock::Image { data, mime_type } => {
                let bytes = BASE64
                    .decode(data)
                    .context("Failed to decode MCP image content")?;
                output.push(ToolResultContent::Image {
                    data: bytes,
                    media_type: mime_type,
                });
            }
            McpContentBlock::Other => {}
        }
    }

    if output.is_empty() && result.is_error {
        output.push(ToolResultContent::Error("MCP tool failed".to_string()));
    }

    Ok(output)
}

fn default_input_schema() -> Value {
    json!({ "type": "object" })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use serde_json::json;

    use super::{McpClient, McpServerConfig};
    use crate::agent::ToolResultContent;

    fn mock_server(mode: &str) -> McpServerConfig {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("core manifest should have a repo parent")
            .to_path_buf();
        let script = repo_root.join("tests").join("fixtures").join("mock_mcp.py");
        let mut args = vec![script.display().to_string()];
        if !mode.is_empty() {
            args.push(mode.to_string());
        }

        McpServerConfig {
            command: "python3".to_string(),
            args,
            env: Default::default(),
            enabled: Some(true),
            timeout_seconds: Some(5),
        }
    }

    #[tokio::test]
    async fn mcp_lists_tools_over_stdio() {
        let client = McpClient::new(mock_server(""));

        let tools = client
            .list_tools()
            .await
            .expect("mock MCP server should list tools");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(
            tools[0].input_schema,
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        );
    }

    #[tokio::test]
    async fn mcp_calls_tool_over_stdio() {
        let client = McpClient::new(mock_server(""));

        let output = client
            .call_tool("echo", json!({ "message": "hello MCP" }))
            .await
            .expect("mock MCP call should succeed");

        assert_eq!(
            output,
            vec![ToolResultContent::Text("echo: hello MCP".to_string())]
        );
    }

    #[tokio::test]
    async fn mcp_malformed_response_errors() {
        let client =
            McpClient::new(mock_server("malformed")).with_timeout(Duration::from_millis(250));

        let error = client
            .list_tools()
            .await
            .expect_err("malformed server output should fail");

        assert!(
            error.to_string().contains("Malformed MCP JSON-RPC"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn mcp_timeout_errors_without_sleeping_test() {
        let client = McpClient::new(mock_server("silent")).with_timeout(Duration::from_millis(100));

        let error = client
            .list_tools()
            .await
            .expect_err("silent server should time out");

        assert!(
            error.to_string().contains("Timed out waiting"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn mcp_missing_command_reports_command_not_found() {
        let config = McpServerConfig {
            command: "codescribe-not-a-real-mcp-binary-xyz".to_string(),
            args: vec![],
            env: Default::default(),
            enabled: Some(true),
            timeout_seconds: Some(2),
        };
        let client = McpClient::new(config);

        let error = client
            .list_tools()
            .await
            .expect_err("a non-existent command must fail discovery");

        assert!(
            error.to_string().contains("command not found"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn mcp_crashed_server_returns_call_error() {
        let client =
            McpClient::new(mock_server("crash-on-call")).with_timeout(Duration::from_millis(250));

        let error = client
            .call_tool("echo", json!({ "message": "boom" }))
            .await
            .expect_err("server crash should become a tool-call error");

        assert!(
            error.to_string().contains("closed stdout"),
            "unexpected error: {error}"
        );
    }
}
