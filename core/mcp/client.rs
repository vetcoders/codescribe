//! MCP client — stdio and Streamable HTTP transports behind one call surface.
//!
//! Every exchange is one-shot: a server is spawned (or an HTTP connection
//! opened), handshaken, used, and shut down per probe or tool call. That costs a
//! spawn each time and buys the property that matters here — a hung or crashed
//! MCP server can never own, stall, or terminate the agent session that invoked
//! it. Failures degrade to a typed error for that one server.
//!
//! ## Key types
//!
//! - [`McpClient`] — the call surface: [`McpClient::probe`],
//!   [`McpClient::list_tools`], [`McpClient::call_tool`]
//! - [`McpConfigFile`] / [`McpServerConfig`] — `~/.codescribe/mcp.json` shape
//! - [`McpProbe`] — handshake identity + live tool list from one exchange

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
use crate::config::keychain::runtime_key;

/// MCP specification revision announced in the `initialize` handshake.
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Per-request ceiling when `mcp.json` names none. Generous because a tool call
/// may legitimately do real work.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// A healthy server answers `initialize` in milliseconds — heavy work belongs
/// to tool calls. A hung server must not stall agent-runtime init for the full
/// request timeout, so the handshake gets its own, much shorter default.
/// An explicit `timeout_seconds` in mcp.json overrides this too.
const DEFAULT_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(5);
/// Budget for each shutdown step (goodbye request, then process wait). A child
/// that outlives it is killed rather than waited on.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);
/// Upper bound on how long a failure-path stderr drain may block. A crashed
/// server exits and yields EOF well within this; a still-alive server that
/// holds stderr open is capped here so the diagnostic never hangs the caller.
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_millis(200);
/// Max characters of collapsed stderr carried into a WARN line.
const STDERR_LOG_MAX_CHARS: usize = 200;
/// Total attempts for a remote exchange, including the first. Bounded so a
/// dead connector surfaces as an error instead of retrying indefinitely.
const REMOTE_RECONNECT_ATTEMPTS: usize = 3;
/// First backoff step; doubles per retry (250 ms, 500 ms).
const REMOTE_RECONNECT_BASE_DELAY: Duration = Duration::from_millis(250);
/// Directories appended to the spawn `PATH`. A GUI-launched app inherits a
/// minimal environment, so without these a Homebrew-installed server would be
/// reported as "command not found" despite being installed.
const FALLBACK_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// Parsed `mcp.json`. Mirrors the `mcpServers` shape shared across MCP hosts,
/// so an existing config file works unchanged.
#[derive(Debug, Clone, Deserialize)]
pub struct McpConfigFile {
    /// Configured servers, keyed by the local name shown in Settings.
    #[serde(rename = "mcpServers", default)]
    pub servers: HashMap<String, McpServerConfig>,
}

impl McpConfigFile {
    /// Read and parse the config at `path`. Errors name the path, since this
    /// surfaces to an operator hand-editing the file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read MCP config {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse MCP config {}", path.display()))
    }

    /// Like [`Self::load`], but a missing file is `Ok(None)` rather than an
    /// error — having no MCP servers configured is a normal state. A file that
    /// exists but does not parse still errors.
    pub fn load_optional(path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }
        Self::load(path).map(Some)
    }
}

/// One server entry. Transport is implied, not declared: a present [`Self::url`]
/// selects remote HTTP, otherwise `command` is spawned over stdio.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    /// Executable to spawn for stdio transport. A bare name is resolved against
    /// the effective `PATH`; ignored when `url` is set.
    #[serde(default)]
    pub command: String,
    /// Arguments passed to `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Non-secret environment variables (PATH, project paths, …).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Secret environment variables stored as Keychain account references.
    /// Keys are env var names; values are Keychain account names (never secret
    /// material). Resolved at spawn via [`crate::mcp::resolve_server_env`].
    #[serde(default)]
    pub env_refs: HashMap<String, String>,
    /// Whether the server participates in tool discovery. `None` is decided by
    /// the caller, not here.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Explicit timeout governing BOTH the handshake and subsequent requests.
    /// When set it overrides the shorter handshake default — an operator who
    /// names a number means it for the whole exchange.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Streamable HTTP endpoint. When present, this server uses remote transport
    /// instead of spawning `command`.
    #[serde(default, alias = "endpoint")]
    pub url: Option<String>,
    /// Keychain account containing the bearer token. The token itself is never
    /// serialized into mcp.json.
    #[serde(default)]
    pub auth_ref: Option<String>,
}

impl McpServerConfig {
    /// Per-request timeout: the configured value, else [`DEFAULT_TIMEOUT`].
    fn timeout(&self) -> Duration {
        self.timeout_seconds
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_TIMEOUT)
    }

    /// Handshake timeout: the configured value, else the much shorter
    /// [`DEFAULT_INITIALIZE_TIMEOUT`], so one hung server cannot stall agent
    /// runtime init for the full request budget.
    fn initialize_timeout(&self) -> Duration {
        self.timeout_seconds
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_INITIALIZE_TIMEOUT)
    }
}

/// One tool as advertised by a server's `tools/list`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct McpTool {
    /// Upstream tool name, as it must be spelled when calling it.
    pub name: String,
    /// Server-supplied description surfaced to the model.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for the arguments. Defaults to an empty object schema when
    /// omitted, so a terse server still yields a callable tool.
    #[serde(rename = "inputSchema", default = "default_input_schema")]
    pub input_schema: Value,
}

/// Server identity advertised in the `initialize` handshake result. All fields
/// are optional: a server may omit `serverInfo` or `protocolVersion`, and the
/// probe still succeeds on the strength of a valid `tools/list`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpHandshake {
    /// MCP revision the server answered with.
    #[serde(rename = "protocolVersion", default)]
    pub protocol_version: Option<String>,
    /// Server's self-reported name and version.
    #[serde(rename = "serverInfo", default)]
    pub server_info: Option<McpServerInfo>,
}

/// Server's self-reported identity from the handshake. Both fields are
/// advisory — nothing routes on them; they exist so Settings can show what
/// actually answered.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpServerInfo {
    /// Server-advertised name.
    #[serde(default)]
    pub name: Option<String>,
    /// Server-advertised version.
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
    /// Identity the server advertised during `initialize`.
    pub handshake: McpHandshake,
    /// Tools the server currently serves.
    pub tools: Vec<McpTool>,
}

/// Canonical config location: `$HOME/.codescribe/mcp.json`. Errors only when
/// `HOME` is unset; the file itself need not exist.
pub fn default_mcp_config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable is not set")?;
    Ok(PathBuf::from(home).join(".codescribe").join("mcp.json"))
}

/// Call surface for one configured MCP server.
///
/// Cheap to construct and holds no connection: each call opens its own
/// transport. Cloning it is fine — clones share nothing but configuration.
#[derive(Debug, Clone)]
pub struct McpClient {
    config: McpServerConfig,
    timeout: Duration,
    initialize_timeout: Duration,
}

impl McpClient {
    /// Build a client, resolving both timeouts from the server config.
    pub fn new(config: McpServerConfig) -> Self {
        let timeout = config.timeout();
        let initialize_timeout = config.initialize_timeout();
        Self {
            config,
            timeout,
            initialize_timeout,
        }
    }

    /// Override BOTH the handshake and request timeouts. Mainly a test seam:
    /// collapsing the two is what lets a test assert a timeout without waiting
    /// out the production budget.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.initialize_timeout = timeout;
        self
    }

    /// The server's live tool list. Thin wrapper over [`Self::probe`] that
    /// discards the handshake identity.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        Ok(self.probe().await?.tools)
    }

    /// Spawn + `initialize` + `tools/list` in one exchange, returning BOTH the
    /// server's advertised identity (name / version / protocol from the
    /// `initialize` handshake) and its live tool list. `list_tools` is a thin
    /// wrapper that keeps only the tools; the Settings health probe uses the full
    /// result to surface real handshake data next to a server.
    pub async fn probe(&self) -> Result<McpProbe> {
        if self.config.url.is_some() {
            return self
                .remote_with_backoff(|mut connection| async move {
                    let handshake = connection.initialize().await?;
                    let response = connection.request("tools/list", json!({})).await?;
                    let tools = parse_tools_list(response)?;
                    Ok(McpProbe { handshake, tools })
                })
                .await;
        }
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

    /// Invoke `name` with `arguments` and return its content blocks.
    ///
    /// Remote servers retry with bounded backoff; stdio servers get one attempt
    /// and their stderr is drained into the failure WARN, so a crash reads as a
    /// diagnosable tool error rather than an opaque one. A server-reported
    /// `isError` becomes [`ToolResultContent::Error`], not an `Err` — the call
    /// reached the tool, and the model should see the failure text.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Vec<ToolResultContent>> {
        if self.config.url.is_some() {
            return self
                .remote_with_backoff(|mut connection| {
                    let name = name.to_string();
                    let arguments = arguments.clone();
                    async move {
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
                })
                .await;
        }
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

    /// Remote calls are isolated from the agent session. A transient connector
    /// failure retries with bounded exponential backoff; exhausting retries
    /// returns a typed connector error to the tool/UI without killing the
    /// session that invoked it.
    async fn remote_with_backoff<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        F: Fn(RemoteConnection) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut last_error = None;
        for attempt in 0..REMOTE_RECONNECT_ATTEMPTS {
            let connection =
                RemoteConnection::connect(&self.config, self.timeout, self.initialize_timeout)?;
            match operation(connection).await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    warn!(
                        "Remote MCP connector '{}' attempt {}/{} failed: {}",
                        self.config.url.as_deref().unwrap_or("<missing>"),
                        attempt + 1,
                        REMOTE_RECONNECT_ATTEMPTS,
                        error.root_cause()
                    );
                    last_error = Some(error);
                    if attempt + 1 < REMOTE_RECONNECT_ATTEMPTS {
                        let multiplier = 1_u32 << attempt;
                        tokio::time::sleep(REMOTE_RECONNECT_BASE_DELAY * multiplier).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("remote MCP connector failed")))
    }
}

/// One Streamable HTTP MCP exchange. We deliberately create this per probe/tool
/// call: the registry contract remains identical to stdio while a dead remote
/// connector cannot own or terminate the surrounding agent session.
struct RemoteConnection {
    /// HTTP client, itself carrying the response timeout.
    client: reqwest::Client,
    /// Validated endpoint URL.
    endpoint: String,
    /// Bearer read from the Keychain; only ever populated over https.
    bearer_token: Option<String>,
    /// Session id echoed back by the server, replayed on later requests.
    session_id: Option<String>,
    /// Next JSON-RPC request id.
    next_id: u64,
    /// Per-request timeout.
    response_timeout: Duration,
    /// Handshake timeout.
    initialize_timeout: Duration,
}

impl RemoteConnection {
    /// Validate the endpoint and prepare an HTTP client.
    ///
    /// Refuses, before any network call: a non-http(s) scheme, credentials
    /// embedded in the URL, and — the one that matters — an `auth_ref` over
    /// plaintext http. That last check fires before the Keychain is even read,
    /// so a misconfigured endpoint cannot put a secret on the wire.
    fn connect(
        config: &McpServerConfig,
        response_timeout: Duration,
        initialize_timeout: Duration,
    ) -> Result<Self> {
        let endpoint = config
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .context("Remote MCP endpoint is empty")?;
        let parsed = reqwest::Url::parse(endpoint)
            .with_context(|| format!("Invalid remote MCP endpoint: {endpoint}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            bail!("Remote MCP endpoint must use http or https");
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            bail!("Remote MCP credentials must be stored in Keychain, not in the endpoint URL");
        }
        let bearer_token = match config.auth_ref.as_deref() {
            Some(account) => {
                // A Keychain secret must never leave the machine in cleartext:
                // plain http would put the bearer on the wire (review P2-17).
                if parsed.scheme() != "https" {
                    bail!(
                        "Remote MCP endpoint with auth_ref must use https — refusing to send the \
                         Keychain bearer token over plaintext http"
                    );
                }
                Some(
                    runtime_key(account).with_context(|| {
                        format!("Missing Keychain token for auth ref '{account}'")
                    })?,
                )
            }
            None => None,
        };
        let client = reqwest::Client::builder()
            .timeout(response_timeout)
            .build()
            .context("Failed to build remote MCP HTTP client")?;
        Ok(Self {
            client,
            endpoint: endpoint.to_string(),
            bearer_token,
            session_id: None,
            next_id: 1,
            response_timeout,
            initialize_timeout,
        })
    }

    /// Run the `initialize` exchange plus the `notifications/initialized`
    /// follow-up. A handshake whose shape is slightly off parses to defaults
    /// rather than failing the probe — identity is advisory, tools are not.
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
        let handshake = serde_json::from_value(result).unwrap_or_default();
        Ok(handshake)
    }

    /// JSON-RPC request under the standard per-request timeout.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(self.response_timeout, method, params)
            .await
    }

    /// JSON-RPC request under an explicit timeout. The response is matched by
    /// request id, so a batched or interleaved reply cannot be mistaken for
    /// this call's answer.
    async fn request_with_timeout(
        &mut self,
        response_timeout: Duration,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let response = timeout(response_timeout, self.post(&message))
            .await
            .with_context(|| {
                format!("Timed out waiting for remote MCP response to '{method}'")
            })??;
        parse_remote_response(response, id, method)
    }

    /// Fire-and-forget JSON-RPC notification (no id, no response awaited).
    async fn notification(&mut self, method: &str, params: Value) -> Result<()> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let _ = self.post(&message).await?;
        Ok(())
    }

    /// POST one message and return the raw body with its content type.
    ///
    /// Captures any `Mcp-Session-Id` for later requests, and treats a non-2xx
    /// status as an error naming the endpoint and code — the shape a connector
    /// outage takes in the Engine tab.
    async fn post(&mut self, message: &Value) -> Result<RemoteResponse> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(message);
        if let Some(token) = &self.bearer_token {
            request = request.bearer_auth(token);
        }
        if let Some(session_id) = &self.session_id {
            request = request.header("Mcp-Session-Id", session_id);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("Remote MCP connector '{}' is unreachable", self.endpoint))?;
        let status = response.status();
        if let Some(session_id) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session_id.to_string());
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response
            .text()
            .await
            .context("Failed reading remote MCP response body")?;
        if !status.is_success() {
            bail!(
                "Remote MCP connector '{}' returned HTTP {}",
                self.endpoint,
                status.as_u16()
            );
        }
        Ok(RemoteResponse { content_type, body })
    }
}

/// Raw HTTP response body plus the content type needed to decide how to read
/// it (Streamable HTTP may answer either plain JSON or an SSE stream).
struct RemoteResponse {
    content_type: String,
    body: String,
}

/// Extract the JSON-RPC result matching `id`, handling both response shapes.
///
/// SSE bodies are reduced to their `data:` frames; plain bodies parse as one
/// message. Selecting by id is what makes an interleaved stream safe to read.
/// A JSON-RPC `error` member becomes an `Err` naming the method.
fn parse_remote_response(response: RemoteResponse, id: u64, method: &str) -> Result<Value> {
    let messages: Vec<Value> = if response.content_type.contains("text/event-stream") {
        response
            .body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .with_context(|| format!("Malformed remote MCP SSE data: {line}"))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        if response.body.trim().is_empty() {
            bail!("Remote MCP server returned an empty response to '{method}'");
        }
        vec![
            serde_json::from_str(response.body.trim())
                .with_context(|| format!("Malformed remote MCP JSON response to '{method}'"))?,
        ]
    };
    let message = messages
        .into_iter()
        .find(|message| message.get("id").and_then(Value::as_u64) == Some(id))
        .with_context(|| format!("Remote MCP response to '{method}' omitted request id {id}"))?;
    if let Some(error) = message.get("error") {
        bail!("MCP request '{method}' failed: {error}");
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
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

/// One spawned MCP server speaking newline-delimited JSON-RPC over stdio.
///
/// Owns the child process for the exchange's lifetime and is consumed by
/// [`Self::shutdown`], so the process cannot outlive the connection value.
struct StdioConnection {
    /// The spawned server process.
    child: Child,
    /// Request pipe, with SIGPIPE disabled per-fd (see `disable_sigpipe`).
    stdin: ChildStdin,
    /// Response pipe, read as lines.
    stdout: Lines<BufReader<ChildStdout>>,
    /// Piped stderr, read only on the failure path (see `drain_stderr`). Taken
    /// out once drained so shutdown does not touch it again.
    stderr: Option<ChildStderr>,
    /// Next JSON-RPC request id.
    next_id: u64,
    /// Per-request timeout.
    response_timeout: Duration,
    /// Handshake timeout.
    initialize_timeout: Duration,
}

impl StdioConnection {
    /// Spawn the server with Keychain-resolved env and an augmented `PATH`.
    ///
    /// Three deliberate choices live here: secrets are resolved from Keychain
    /// refs at spawn time (never stored in `mcp.json`); a `NotFound` spawn error
    /// is rewritten into a `command not found` message naming the searched
    /// `PATH`, because that is the failure operators actually hit; and stderr is
    /// piped rather than nulled so a server that dies mid-handshake can still
    /// explain itself.
    async fn spawn(
        config: &McpServerConfig,
        response_timeout: Duration,
        initialize_timeout: Duration,
    ) -> Result<Self> {
        let resolved_env =
            crate::mcp::secret_migration::resolve_server_env(&config.env, &config.env_refs)
                .with_context(|| {
                    format!(
                        "Failed to resolve Keychain env_refs for MCP server '{}'",
                        config.command
                    )
                })?;
        let effective_path = effective_mcp_path(resolved_env.get("PATH").map(String::as_str));
        let resolved_command = resolve_command(&config.command, &effective_path);
        let mut command = Command::new(&resolved_command);
        command
            .args(&config.args)
            .envs(&resolved_env)
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

    /// Run the `initialize` exchange plus the `notifications/initialized`
    /// follow-up, under the shorter handshake timeout.
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

    /// JSON-RPC request under the standard per-request timeout.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(self.response_timeout, method, params)
            .await
    }

    /// JSON-RPC request under an explicit timeout.
    ///
    /// Reads lines until one carries the matching id, skipping the server's own
    /// notifications and log chatter rather than mistaking them for the answer.
    /// EOF before a reply means the server died mid-exchange and surfaces as a
    /// "closed stdout" error.
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

    /// Fire-and-forget JSON-RPC notification (no id, no response awaited).
    async fn notification(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_message(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    /// Serialize and write one newline-delimited JSON-RPC message, flushing it.
    /// A write to a dead peer returns EPIPE rather than raising SIGPIPE — see
    /// [`disable_sigpipe`].
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

    /// Consume the connection and stop the server.
    ///
    /// Sends the JSON-RPC goodbye only to a process still alive, then closes
    /// stdin and waits — killing the child if it overruns [`SHUTDOWN_TIMEOUT`].
    /// Skipping the goodbye for an already-exited child is the point: writing
    /// into its closed stdin is EPIPE noise at best, and in a host that never
    /// ran Rust's `main` signal setup, fatal at worst.
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

/// No-op outside macOS: `F_SETNOSIGPIPE` is a Darwin-specific fcntl.
#[cfg(not(target_os = "macos"))]
fn disable_sigpipe(_stdin: &ChildStdin) {}

/// Build the `PATH` a spawned server sees: the server's own configured `PATH`
/// first, then the process `PATH`, then the user bins and system fallbacks.
///
/// Order is precedence, and duplicates are dropped. This exists because a
/// GUI-launched app inherits a minimal `PATH` — without the fallbacks, servers
/// that are installed would report as missing.
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

/// Resolve a bare command name to the first executable match on
/// `effective_path`. A command containing `/` is already a path and is returned
/// unchanged; an unresolvable name is returned as-is so the spawn error names
/// what the operator actually configured.
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

/// Split a `PATH`-style string and append each entry, preserving order.
fn push_path_entries(entries: &mut Vec<PathBuf>, path: &OsStr) {
    for entry in std::env::split_paths(path) {
        push_unique_path(entries, entry);
    }
}

/// Append `path` unless it is empty or already present, so earlier entries keep
/// their precedence.
fn push_unique_path(entries: &mut Vec<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() || entries.iter().any(|existing| existing == &path) {
        return;
    }
    entries.push(path);
}

/// Whether `path` is a regular file with any execute bit set. Unreadable
/// metadata counts as "not executable" rather than erroring.
#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Non-Unix fallback: permission bits are not consulted, only file-ness.
#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// Wire shape of a `tools/list` result. A server advertising no tools is valid,
/// hence the default.
#[derive(Debug, Deserialize)]
struct ToolsListResult {
    #[serde(default)]
    tools: Vec<McpTool>,
}

/// Decode a `tools/list` result into the tool list.
fn parse_tools_list(value: Value) -> Result<Vec<McpTool>> {
    let result: ToolsListResult =
        serde_json::from_value(value).context("Failed to parse MCP tools/list result")?;
    Ok(result.tools)
}

/// Wire shape of a `tools/call` result: content blocks plus the server's own
/// success flag.
#[derive(Debug, Deserialize)]
struct ToolCallResult {
    #[serde(default)]
    content: Vec<McpContentBlock>,
    #[serde(rename = "isError", default)]
    is_error: bool,
}

/// One content block from a tool result, tagged by its `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum McpContentBlock {
    /// Plain text payload.
    Text { text: String },
    /// Base64 image payload with its MIME type.
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Any block type this client does not model. Present so an unknown block
    /// is skipped rather than failing the whole result — future MCP additions
    /// must not break existing tool calls.
    #[serde(other)]
    Other,
}

/// Convert a `tools/call` result into the agent's content blocks.
///
/// Text under `isError` becomes [`ToolResultContent::Error`]; images are
/// base64-decoded; unmodelled blocks are dropped. An error result carrying no
/// content still yields one error block, so a failure is never silent.
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

/// Empty object schema used when a server omits `inputSchema`, keeping such a
/// tool callable instead of rejecting it.
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

    /// Backstop for the stdio tests whose claim is *what* the failure says, not
    /// how fast it arrives.
    ///
    /// These tests never spend this budget on the green path: the mock server
    /// answers, emits garbage, or dies the moment it is up, so each one returns
    /// on a pipe event within tens of milliseconds. The value exists only to
    /// bound a hang.
    ///
    /// It must stay far out of reach of machine load, because the budget covers
    /// `spawn(python3) + initialize`, not merely the wait for a response.
    /// Measured on an idle host that handshake is ~25 ms (n=12, min 23 / max
    /// 28) — a sub-second backstop is a bet that a loaded box is never ten
    /// times slower at starting an interpreter. When that bet loses, the
    /// deadline wins the race and the test panics with
    /// `unexpected error: Timed out waiting for MCP response to 'initialize'`:
    /// a red that names the JSON-RPC layer while the JSON-RPC layer is healthy.
    ///
    /// A tight deadline is legitimate only where the deadline *is* the claim —
    /// see `mcp_timeout_errors_without_sleeping_test`.
    const CONTENT_ASSERTION_BACKSTOP: Duration = Duration::from_secs(10);

    /// Config pointing at `tests/fixtures/mock_mcp.py`, whose `mode` argument
    /// selects the behaviour under test (empty = well-behaved; `malformed`,
    /// `silent`, `crash-on-call`, `exit-before-initialize` = the failure shapes).
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
            env_refs: Default::default(),
            enabled: Some(true),
            timeout_seconds: Some(5),
            url: None,
            auth_ref: None,
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
    async fn mcp_lists_tools_over_remote_streamable_http() {
        use mockito::Matcher;

        let mut server = mockito::Server::new_async().await;
        let initialize = server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(json!({"method": "initialize"})))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("Mcp-Session-Id", "codescribe-test-session")
            .with_body(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2025-06-18",
                        "serverInfo": {"name": "remote-mock", "version": "1.0"}
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let initialized = server
            .mock("POST", "/mcp")
            .match_header("Mcp-Session-Id", "codescribe-test-session")
            .match_body(Matcher::PartialJson(
                json!({"method": "notifications/initialized"}),
            ))
            .with_status(202)
            .create_async()
            .await;
        let list = server
            .mock("POST", "/mcp")
            .match_header("Mcp-Session-Id", "codescribe-test-session")
            .match_body(Matcher::PartialJson(json!({"method": "tools/list"})))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(
                "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"remote_search\",\"inputSchema\":{\"type\":\"object\"}}]}}\n\n",
            )
            .create_async()
            .await;

        let client = McpClient::new(McpServerConfig {
            command: String::new(),
            args: vec![],
            env: Default::default(),
            env_refs: Default::default(),
            enabled: Some(true),
            timeout_seconds: Some(2),
            url: Some(format!("{}/mcp", server.url())),
            auth_ref: None,
        });
        let probe = client.probe().await.expect("remote MCP probe");
        assert_eq!(
            probe.handshake.server_name().as_deref(),
            Some("remote-mock")
        );
        assert_eq!(probe.tools[0].name, "remote_search");
        initialize.assert_async().await;
        initialized.assert_async().await;
        list.assert_async().await;
    }

    #[tokio::test]
    async fn remote_mcp_refuses_bearer_over_plaintext_http() {
        // review P2-17: a Keychain secret must not be sent in cleartext. The
        // failure must land before the token is even read from the Keychain.
        let client = McpClient::new(McpServerConfig {
            command: String::new(),
            args: vec![],
            env: Default::default(),
            env_refs: Default::default(),
            enabled: Some(true),
            timeout_seconds: Some(2),
            url: Some("http://mcp.example.invalid/mcp".to_string()),
            auth_ref: Some("codescribe-test-account".to_string()),
        });
        let error = client
            .probe()
            .await
            .expect_err("http + auth_ref must be refused");
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("https"),
            "error must name the https requirement, got: {rendered}"
        );
    }

    #[tokio::test]
    async fn remote_mcp_reconnects_with_backoff_without_losing_the_call() {
        use tiny_http::{Header, Response, Server, StatusCode};

        let server = Server::http("127.0.0.1:0").expect("bind mock MCP");
        let endpoint = format!("http://{}/mcp", server.server_addr());
        let worker = std::thread::spawn(move || {
            // Simulate the connector being down for two initialize attempts,
            // then coming back inside the client's bounded backoff window.
            for _ in 0..2 {
                let request = server.recv().expect("failed initialize request");
                request
                    .respond(Response::empty(StatusCode(503)))
                    .expect("503 response");
            }

            let request = server.recv().expect("reconnected initialize");
            let response = Response::from_string(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2025-06-18",
                        "serverInfo": {"name": "reconnected"}
                    }
                })
                .to_string(),
            )
            .with_header(
                Header::from_bytes("content-type", "application/json").expect("content type"),
            )
            .with_header(
                Header::from_bytes("Mcp-Session-Id", "reconnected-session")
                    .expect("session header"),
            );
            request.respond(response).expect("initialize response");

            let request = server.recv().expect("initialized notification");
            request
                .respond(Response::empty(StatusCode(202)))
                .expect("notification response");

            let request = server.recv().expect("tools/list");
            request
                .respond(
                    Response::from_string(
                        json!({
                            "jsonrpc": "2.0",
                            "id": 2,
                            "result": {
                                "tools": [{
                                    "name": "after_restart",
                                    "inputSchema": {"type": "object"}
                                }]
                            }
                        })
                        .to_string(),
                    )
                    .with_header(
                        Header::from_bytes("content-type", "application/json")
                            .expect("content type"),
                    ),
                )
                .expect("tools response");
        });

        let client = McpClient::new(McpServerConfig {
            command: String::new(),
            args: vec![],
            env: Default::default(),
            env_refs: Default::default(),
            enabled: Some(true),
            timeout_seconds: Some(2),
            url: Some(endpoint),
            auth_ref: None,
        });
        let tools = client
            .list_tools()
            .await
            .expect("connector should recover during backoff");
        assert_eq!(tools[0].name, "after_restart");
        worker.join().expect("mock server thread");
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
            McpClient::new(mock_server("malformed")).with_timeout(CONTENT_ASSERTION_BACKSTOP);

        let error = client
            .list_tools()
            .await
            .expect_err("malformed server output should fail");

        assert!(
            error.to_string().contains("Malformed MCP JSON-RPC"),
            "unexpected error: {error}"
        );
    }

    /// The one test in this module that may keep a tight deadline: here the
    /// deadline IS the claim. The `silent` server never answers, so a slow
    /// spawn cannot change the outcome — it still times out, which is exactly
    /// what is asserted. Do not migrate this to
    /// [`CONTENT_ASSERTION_BACKSTOP`]; that would trade a 100 ms test for a
    /// 10 s one and assert nothing new.
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
            env_refs: Default::default(),
            enabled: Some(true),
            timeout_seconds: Some(2),
            url: None,
            auth_ref: None,
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
            McpClient::new(mock_server("crash-on-call")).with_timeout(CONTENT_ASSERTION_BACKSTOP);

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
            .with_timeout(CONTENT_ASSERTION_BACKSTOP);

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

        // The backstop is deliberately out of reach of machine load. This test
        // asserts only `is_err()`, so a fired deadline would satisfy it without
        // the client ever writing to the dead pipe — the guard would go green
        // without exercising the hole it exists to falsify. Here a tight clock
        // does not produce a false red, it produces a vacuous pass, which is
        // the worse of the two.
        let client = McpClient::new(mock_server("exit-before-initialize"))
            .with_timeout(CONTENT_ASSERTION_BACKSTOP);
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
