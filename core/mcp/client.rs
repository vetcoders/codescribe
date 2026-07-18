use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::agent::ToolResultContent;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// A healthy server answers `initialize` in milliseconds — heavy work belongs
/// to tool calls. A hung server must not stall agent-runtime init for the full
/// request timeout, so the handshake gets its own, much shorter default.
/// An explicit `timeout_seconds` in mcp.json overrides this too.
const DEFAULT_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);
/// Upper bound on how long a failure-path stderr drain may block. A crashed
/// server exits and yields EOF well within this; a still-alive server that
/// holds stderr open is capped here so the diagnostic never hangs the caller.
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_millis(200);
/// Max characters of collapsed stderr carried into a WARN line.
const STDERR_LOG_MAX_CHARS: usize = 200;
const FALLBACK_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

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

    fn initialize_timeout(&self) -> Duration {
        self.timeout_seconds
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_INITIALIZE_TIMEOUT)
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

/// Server identity advertised in the `initialize` handshake result. All fields
/// are optional: a server may omit `serverInfo` or `protocolVersion`, and the
/// probe still succeeds on the strength of a valid `tools/list`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpHandshake {
    #[serde(rename = "protocolVersion", default)]
    pub protocol_version: Option<String>,
    #[serde(rename = "serverInfo", default)]
    pub server_info: Option<McpServerInfo>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpServerInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

impl McpHandshake {
    /// Server-advertised name, if any (e.g. `prview.mcp.v1`).
    pub fn server_name(&self) -> Option<String> {
        self.server_info.as_ref().and_then(|info| info.name.clone())
    }

    /// Server-advertised version, if any (e.g. `0.4.0`).
    pub fn server_version(&self) -> Option<String> {
        self.server_info
            .as_ref()
            .and_then(|info| info.version.clone())
    }
}

/// Full result of a health probe: the handshake identity plus the live tools.
#[derive(Debug, Clone, Default)]
pub struct McpProbe {
    pub handshake: McpHandshake,
    pub tools: Vec<McpTool>,
}

pub fn default_mcp_config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable is not set")?;
    Ok(PathBuf::from(home).join(".codescribe").join("mcp.json"))
}

#[derive(Debug, Clone)]
pub struct McpClient {
    config: McpServerConfig,
    timeout: Duration,
    initialize_timeout: Duration,
}

impl McpClient {
    pub fn new(config: McpServerConfig) -> Self {
        let timeout = config.timeout();
        let initialize_timeout = config.initialize_timeout();
        Self {
            config,
            timeout,
            initialize_timeout,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.initialize_timeout = timeout;
        self
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        Ok(self.probe().await?.tools)
    }

    /// Spawn + `initialize` + `tools/list` in one exchange, returning BOTH the
    /// server's advertised identity (name / version / protocol from the
    /// `initialize` handshake) and its live tool list. `list_tools` is a thin
    /// wrapper that keeps only the tools; the Settings health probe uses the full
    /// result to surface real handshake data next to a server.
    pub async fn probe(&self) -> Result<McpProbe> {
        let mut connection =
            match StdioConnection::spawn(&self.config, self.timeout, self.initialize_timeout).await
            {
                Ok(connection) => connection,
                Err(error) => {
                    warn!(
                        "MCP server '{}' failed to spawn: {error}",
                        self.config.command
                    );
                    return Err(error);
                }
            };
        let result = async {
            let handshake = connection.initialize().await?;
            let response = connection.request("tools/list", json!({})).await?;
            let tools = parse_tools_list(response)?;
            Ok(McpProbe { handshake, tools })
        }
        .await;
        if let Err(error) = &result {
            let stderr = connection.drain_stderr().await;
            warn_handshake_failure(&self.config.command, "tools/list", error, &stderr);
        }
        let shutdown = connection.shutdown().await;
        if let Err(error) = shutdown {
            debug!("MCP shutdown after probe failed: {error}");
        }
        result
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Vec<ToolResultContent>> {
        let mut connection =
            match StdioConnection::spawn(&self.config, self.timeout, self.initialize_timeout).await
            {
                Ok(connection) => connection,
                Err(error) => {
                    warn!(
                        "MCP server '{}' failed to spawn for tool '{name}': {error}",
                        self.config.command
                    );
                    return Err(error);
                }
            };
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
        if let Err(error) = &result {
            let stderr = connection.drain_stderr().await;
            warn_handshake_failure(&self.config.command, "tools/call", error, &stderr);
        }
        let shutdown = connection.shutdown().await;
        if let Err(error) = shutdown {
            debug!("MCP shutdown after tools/call failed: {error}");
        }
        result
    }
}

/// Emit a WARN for a spawn-survived-but-handshake/call-failed MCP exchange,
/// enriched with the process stderr (already collapsed and truncated) when the
/// server wrote anything before failing.
fn warn_handshake_failure(command: &str, phase: &str, error: &anyhow::Error, stderr: &str) {
    if stderr.is_empty() {
        warn!("MCP server '{command}' {phase} failed: {error}");
    } else {
        warn!("MCP server '{command}' {phase} failed: {error} — stderr: {stderr}");
    }
}

struct StdioConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    /// Piped stderr, read only on the failure path (see `drain_stderr`). Taken
    /// out once drained so shutdown does not touch it again.
    stderr: Option<ChildStderr>,
    next_id: u64,
    response_timeout: Duration,
    initialize_timeout: Duration,
}

impl StdioConnection {
    async fn spawn(
        config: &McpServerConfig,
        response_timeout: Duration,
        initialize_timeout: Duration,
    ) -> Result<Self> {
        let effective_path = effective_mcp_path(config.env.get("PATH").map(String::as_str));
        let resolved_command = resolve_command(&config.command, &effective_path);
        let mut command = Command::new(&resolved_command);
        command
            .args(&config.args)
            .envs(&config.env)
            .env("PATH", &effective_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Pipe (not null) so a spawn-survived-but-handshake-failed server's
            // stderr can be surfaced in a WARN. One-shot per call, drained on the
            // failure path and closed at shutdown, so it cannot back-pressure a
            // healthy call.
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|err| {
            // Give the most common failure a concrete, actionable reason instead
            // of a generic spawn error — this string surfaces in the Engine tab.
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "command not found: '{}' (searched PATH: {})",
                    config.command,
                    effective_path.to_string_lossy()
                )
            } else {
                anyhow::Error::new(err)
                    .context(format!("Failed to spawn MCP server '{}'", config.command))
            }
        })?;

        let stdin = child
            .stdin
            .take()
            .context("MCP server stdin was not piped")?;
        // Rust's std runtime only ignores SIGPIPE when a Rust `main` runs; this
        // code also lives inside the codescribe-ffi dylib hosted by the SwiftUI
        // app, where SIGPIPE keeps its default FATAL disposition. A server that
        // dies before/mid-exchange (e.g. SIGKILLed by a code-signature check)
        // closes its end of the stdin pipe, and the next write would kill the
        // whole host process without a crash report. F_SETNOSIGPIPE makes such
        // writes fail with EPIPE instead, which surfaces as a normal Result
        // error and degrades just that server.
        disable_sigpipe(&stdin);
        let stdout = child
            .stdout
            .take()
            .context("MCP server stdout was not piped")?;
        let stderr = child.stderr.take();

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            stderr,
            next_id: 1,
            response_timeout,
            initialize_timeout,
        })
    }

    /// Best-effort read of whatever the server wrote to stderr, collapsed to a
    /// single line and truncated for logging. Bounded by `STDERR_DRAIN_TIMEOUT`
    /// so a still-running child that holds stderr open cannot block the caller.
    async fn drain_stderr(&mut self) -> String {
        let Some(stderr) = self.stderr.take() else {
            return String::new();
        };
        let mut reader = BufReader::new(stderr);
        let mut buffer = Vec::new();
        // On timeout the read future is dropped; bytes already read stay in
        // `buffer`, which is enough for a diagnostic snippet.
        let _ = timeout(STDERR_DRAIN_TIMEOUT, reader.read_to_end(&mut buffer)).await;
        truncate_stderr(&String::from_utf8_lossy(&buffer))
    }

    async fn initialize(&mut self) -> Result<McpHandshake> {
        let result = self
            .request_with_timeout(
                self.initialize_timeout,
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
            .await?;
        // A well-formed server returns `serverInfo` + `protocolVersion`; a slightly
        // off shape must not fail the whole probe, so parse leniently to defaults.
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(self.response_timeout, method, params)
            .await
    }

    async fn request_with_timeout(
        &mut self,
        response_timeout: Duration,
        method: &str,
        params: Value,
    ) -> Result<Value> {
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
            let line = timeout(response_timeout, self.stdout.next_line())
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
        // A child that already exited (we saw EOF / a failed exchange) gets no
        // JSON-RPC goodbye: writing into its closed stdin is at best EPIPE
        // noise, at worst a fatal SIGPIPE in a host process that never ran
        // Rust's main-thread signal setup (see `disable_sigpipe`).
        let already_exited = matches!(self.child.try_wait(), Ok(Some(_)));
        if !already_exited {
            let _ = timeout(SHUTDOWN_TIMEOUT, self.request("shutdown", json!({}))).await;
            let _ = self
                .notification("notifications/exit", json!({}))
                .await
                .map_err(|error| debug!("MCP exit notification failed: {error}"));
        }
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

/// Mark the child's stdin pipe so writes to a dead peer return EPIPE instead
/// of raising SIGPIPE. Per-fd (`F_SETNOSIGPIPE`) on purpose: it protects the
/// MCP exchange without mutating the host process' signal table.
#[cfg(target_os = "macos")]
fn disable_sigpipe(stdin: &ChildStdin) {
    use std::os::fd::AsRawFd;

    // Darwin `sys/fcntl.h`: `#define F_SETNOSIGPIPE 73` — the libc crate does
    // not export this per-fd fcntl command (only the socket-level
    // `SO_NOSIGPIPE`), so pin the value here.
    const F_SETNOSIGPIPE: libc::c_int = 73;

    // SAFETY: fcntl on an fd we own for the child's lifetime; F_SETNOSIGPIPE
    // only flips a per-fd flag. A failure leaves the old behavior in place and
    // is tolerable — the try_wait guard in `shutdown` still narrows exposure.
    let _ = unsafe { libc::fcntl(stdin.as_raw_fd(), F_SETNOSIGPIPE, 1) };
}

#[cfg(not(target_os = "macos"))]
fn disable_sigpipe(_stdin: &ChildStdin) {}

fn effective_mcp_path(config_path: Option<&str>) -> OsString {
    let mut entries = Vec::new();

    if let Some(path) = config_path {
        push_path_entries(&mut entries, OsStr::new(path));
    }
    if let Some(path) = std::env::var_os("PATH") {
        push_path_entries(&mut entries, &path);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        push_unique_path(&mut entries, home.join(".cargo").join("bin"));
        push_unique_path(&mut entries, home.join(".local").join("bin"));
    }
    for path in FALLBACK_PATHS {
        push_unique_path(&mut entries, PathBuf::from(path));
    }

    std::env::join_paths(entries).unwrap_or_else(|_| {
        OsString::from("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
    })
}

fn resolve_command(command: &str, effective_path: &OsStr) -> OsString {
    if command.contains('/') {
        return OsString::from(command);
    }

    for dir in std::env::split_paths(effective_path) {
        let candidate = dir.join(command);
        if is_executable_file(&candidate) {
            return candidate.into_os_string();
        }
    }

    OsString::from(command)
}

fn push_path_entries(entries: &mut Vec<PathBuf>, path: &OsStr) {
    for entry in std::env::split_paths(path) {
        push_unique_path(entries, entry);
    }
}

fn push_unique_path(entries: &mut Vec<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() || entries.iter().any(|existing| existing == &path) {
        return;
    }
    entries.push(path);
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
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

/// Collapse multi-line/whitespace-heavy stderr into one truncated log-friendly
/// line.
fn truncate_stderr(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= STDERR_LOG_MAX_CHARS {
        return collapsed;
    }
    collapsed
        .chars()
        .take(STDERR_LOG_MAX_CHARS.saturating_sub(3))
        .collect::<String>()
        + "..."
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    use serde_json::json;
    #[cfg(target_os = "macos")]
    use serial_test::serial;
    use tempfile::TempDir;

    use super::{McpClient, McpServerConfig, effective_mcp_path, resolve_command};
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
    async fn mcp_probe_captures_handshake_identity() {
        let client = McpClient::new(mock_server(""));

        let probe = client
            .probe()
            .await
            .expect("mock MCP server should complete the handshake");

        assert_eq!(probe.tools.len(), 1);
        assert_eq!(probe.handshake.server_name().as_deref(), Some("mock-mcp"));
        assert_eq!(probe.handshake.server_version().as_deref(), Some("0.1.0"));
        assert_eq!(
            probe.handshake.protocol_version.as_deref(),
            Some("2025-06-18")
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

    #[test]
    fn mcp_effective_path_includes_gui_missing_user_bins() {
        let path = effective_mcp_path(None);
        let path_string = path.to_string_lossy();

        assert!(
            path_string.contains("/opt/homebrew/bin"),
            "expected Homebrew fallback in PATH, got {path_string}"
        );
        assert!(
            path_string.contains("/usr/bin"),
            "expected system fallback in PATH, got {path_string}"
        );

        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            assert!(
                path_string.contains(&home.join(".cargo/bin").to_string_lossy().to_string()),
                "expected cargo bin fallback in PATH, got {path_string}"
            );
            assert!(
                path_string.contains(&home.join(".local/bin").to_string_lossy().to_string()),
                "expected local bin fallback in PATH, got {path_string}"
            );
        }
    }

    #[test]
    fn mcp_resolves_bare_command_from_config_path() {
        let temp = TempDir::new().expect("tempdir");
        let command_path = temp.path().join("codescribe-test-mcp");
        fs::write(&command_path, "#!/bin/sh\nexit 0\n").expect("write executable");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&command_path).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&command_path, perms).expect("chmod");
        }

        let temp_path = temp.path().to_string_lossy().to_string();
        let effective_path = effective_mcp_path(Some(&temp_path));
        let resolved = resolve_command("codescribe-test-mcp", &effective_path);

        assert_eq!(PathBuf::from(resolved), command_path);
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

    /// Incident 2026-07-16 13:52 shape: the server process dies before
    /// answering `initialize` (code-signature SIGKILL). The probe must degrade
    /// to an error — never panic, never take the process down.
    #[tokio::test]
    async fn mcp_server_dead_before_initialize_degrades_to_error() {
        let client = McpClient::new(mock_server("exit-before-initialize"))
            .with_timeout(Duration::from_millis(500));

        let error = client
            .list_tools()
            .await
            .expect_err("a dead-at-start server must fail discovery");

        // Depending on the exit/write race this surfaces as EOF-before-response
        // or as a failed pipe write; both are acceptable degradations.
        let message = format!("{error:#}");
        assert!(
            message.contains("closed stdout") || message.contains("Failed to write MCP message"),
            "unexpected error: {message}"
        );
    }

    /// Falsification harness for the SIGPIPE hole: cargo-test binaries run with
    /// SIGPIPE ignored (Rust `main` setup), which masks the exact condition that
    /// killed the Swift-hosted app. Restore the default FATAL disposition for
    /// the exchange — without the `F_SETNOSIGPIPE` + shutdown guards this test
    /// does not fail, it kills the whole test process.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[serial]
    async fn mcp_dead_server_survives_default_sigpipe_disposition() {
        // SAFETY: process-wide signal disposition swap, serialized via `serial`
        // and restored before the test returns.
        let previous = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

        let client = McpClient::new(mock_server("exit-before-initialize"))
            .with_timeout(Duration::from_millis(500));
        let result = client.list_tools().await;

        // SAFETY: restores the disposition captured above.
        unsafe { libc::signal(libc::SIGPIPE, previous) };

        assert!(
            result.is_err(),
            "dead server must degrade to an error while the process survives"
        );
    }

    #[test]
    fn mcp_initialize_timeout_defaults_shorter_than_request_timeout() {
        let config = mock_server("");
        let no_override = McpServerConfig {
            timeout_seconds: None,
            ..config.clone()
        };
        assert_eq!(no_override.initialize_timeout(), Duration::from_secs(5));
        assert_eq!(no_override.timeout(), Duration::from_secs(30));

        // An explicit timeout_seconds governs BOTH phases — user truth wins.
        assert_eq!(config.initialize_timeout(), Duration::from_secs(5));
        assert_eq!(config.timeout(), Duration::from_secs(5));
    }
}
