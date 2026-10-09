//! MCP server discovery, tool registration, and the readiness probes that
//! report on both.
//!
//! Two responsibilities live here:
//!
//! - **Registration.** [`register`] reads the user's `mcp.json`, probes every
//!   enabled server in parallel and in isolation, and exposes each discovered
//!   tool under an `mcp__<server>__<tool>` name. One dead or hung server costs
//!   only its own timeout: the session starts degraded, never dead. Desktop
//!   Commander gets a hardened profile on top — per-tool risk classification,
//!   workspace-root path validation, and secret redaction in approval previews.
//! - **Reporting.** The Settings Engine tab and the onboarding readiness step
//!   read [`probe_mcp_status`] and [`probe_agentic_readiness`]. Per the C4
//!   decision, MCP servers are operator tooling and are INFORMATIONAL only:
//!   readiness is decided solely by the core capability gate
//!   ([`CoreReadiness`]), so a missing or broken `mcp.json` can never sink a
//!   working agent.
//!
//! Both read one evidence owner, [`McpEvidence`]: the last runtime discovery
//! (what the agent registered) and the last Settings connection test per
//! server (what answered when the user pressed Test). The two never merge,
//! each is pinned to the exact `mcp.json` entry it was gathered against, and
//! each carries the evidence sequence number that orders them.
//! Contract: `docs/MCP_DIAGNOSTICS.md`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{
    ToolCallPreview, ToolDefinition, ToolExecutionPolicy, ToolInputValidator, ToolOrigin,
    ToolRegistry, ToolResultContent, ToolRisk,
};
use codescribe_core::config::RuntimeSettingsSnapshot;
use codescribe_core::config::settings::{
    DEFAULT_AGENT_WORKSPACE_ROOT, normalize_agent_workspace_roots,
};
use codescribe_core::mcp::{
    McpClient, McpConfigFile, McpProbeSummary, McpServerConfig, McpTool,
    probe_server_config_blocking,
};
use tracing::{info, warn};

use super::path_policy;

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

/// What the last discovery pass learned about one server, pinned to the
/// config entry it ran against.
#[derive(Debug, Clone)]
struct RuntimeEvidence {
    config: McpServerConfig,
    outcome: ServerRuntime,
    /// `serverInfo.name` from the handshake, when the server sent one.
    advertised: Option<String>,
    /// [`McpEvidence::events`] value when this pass was recorded.
    seq: u64,
}

/// The last Settings connection test of one server, pinned to the config
/// entry it ran against. A one-shot probe: it proves reachability, never that
/// the agent registered the tools.
#[derive(Debug, Clone)]
struct TestEvidence {
    config: McpServerConfig,
    outcome: std::result::Result<McpProbeSummary, String>,
    /// [`McpEvidence::events`] value when this test was recorded.
    seq: u64,
}

/// The one owner of MCP connection evidence, keyed by server name. Runtime
/// discovery and connection tests stay separate ledgers; readers accept an
/// entry only while its pinned config equals the current `mcp.json` entry, so a
/// renamed or edited server never inherits evidence it did not earn.
#[derive(Debug, Clone, Default)]
struct McpEvidence {
    runtime: BTreeMap<String, RuntimeEvidence>,
    tests: BTreeMap<String, TestEvidence>,
    /// Count of recorded discovery passes and tests. Each record takes the
    /// next value under the store lock, so `seq` orders evidence by when it
    /// was recorded — no clock involved.
    events: u64,
}

impl McpEvidence {
    /// Discovery outcome for `name`, when it ran against this exact entry.
    fn runtime_for(&self, name: &str, config: &McpServerConfig) -> Option<&RuntimeEvidence> {
        self.runtime
            .get(name)
            .filter(|evidence| evidence.config == *config)
    }

    /// Connection test for `name`, when it ran against this exact entry.
    fn test_for(&self, name: &str, config: &McpServerConfig) -> Option<&TestEvidence> {
        self.tests
            .get(name)
            .filter(|evidence| evidence.config == *config)
    }

    /// Identity the server advertised for this exact entry: discovery first,
    /// then the last successful connection test.
    fn advertised_for(&self, name: &str, config: &McpServerConfig) -> Option<&str> {
        self.runtime_for(name, config)
            .and_then(|evidence| evidence.advertised.as_deref())
            .or_else(|| {
                self.test_for(name, config)
                    .and_then(|evidence| evidence.outcome.as_ref().ok())
                    .and_then(|summary| summary.server_name.as_deref())
            })
            .filter(|identity| !identity.trim().is_empty())
    }

    /// Next evidence sequence number.
    fn next_seq(&mut self) -> u64 {
        self.events += 1;
        self.events
    }

    /// One server's state from the evidence that still holds for its entry.
    ///
    /// `enabled: false` wins. Registration (what the agent holds) is always
    /// reported when it exists; a connection test recorded after it is
    /// reported alongside when the two disagree — a later failed test never
    /// hides registered tools, and a later passing test never claims a
    /// registration that failed. A test older than the registration is
    /// superseded by it. Without registration the last test decides.
    fn server_state(&self, name: &str, config: &McpServerConfig) -> ServerState {
        if !config.enabled.unwrap_or(true) {
            return ServerState::Disabled;
        }
        let runtime = self.runtime_for(name, config);
        let test = self
            .test_for(name, config)
            .filter(|test| runtime.is_none_or(|runtime| test.seq > runtime.seq))
            .map(|test| &test.outcome);
        match (runtime.map(|runtime| &runtime.outcome), test) {
            (Some(ServerRuntime::Disabled), _) => ServerState::Disabled,
            (Some(ServerRuntime::Tools(tools)), Some(Err(reason))) => {
                ServerState::LiveLastTestFailed {
                    tools: *tools,
                    reason: reason.clone(),
                }
            }
            (Some(ServerRuntime::Tools(tools)), _) => ServerState::Live(*tools),
            (Some(ServerRuntime::Failed(reason)), Some(Ok(summary))) => {
                ServerState::FailedLastTestPassed {
                    reason: reason.clone(),
                    tools: summary.tool_count,
                }
            }
            (Some(ServerRuntime::Failed(reason)), _) => ServerState::Failed(reason.clone()),
            (None, Some(Ok(summary))) => ServerState::Reachable(summary.tool_count),
            (None, Some(Err(reason))) => ServerState::Unreachable(reason.clone()),
            (None, None) => ServerState::Configured,
        }
    }
}

/// Evidence-backed state of one configured server.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerState {
    /// Registered by the agent with this many tools.
    Live(usize),
    /// Registered `tools`, but a later connection test failed (`reason`).
    LiveLastTestFailed {
        tools: usize,
        reason: String,
    },
    /// Registration failed (`reason`), but a later connection test answered
    /// with `tools`; the agent still holds none of them.
    FailedLastTestPassed {
        reason: String,
        tools: usize,
    },
    /// Connection test answered with this many tools; not registered yet.
    Reachable(usize),
    /// No evidence for the current entry yet.
    Configured,
    /// Runtime discovery failed (reason).
    Failed(String),
    /// Connection test failed (reason); no runtime registration.
    Unreachable(String),
    Disabled,
}

impl ServerState {
    /// Selection order when several servers match one operator tool: the
    /// strongest proof of a working server first.
    fn rank(&self) -> u8 {
        match self {
            Self::Live(_) => 0,
            Self::LiveLastTestFailed { .. } => 1,
            Self::Reachable(_) | Self::FailedLastTestPassed { .. } => 2,
            Self::Configured => 3,
            Self::Failed(_) => 4,
            Self::Unreachable(_) => 5,
            Self::Disabled => 6,
        }
    }

    /// Plain-English name used in selection explanations and logs.
    fn describe(&self) -> &'static str {
        match self {
            Self::Live(_) => "live",
            Self::LiveLastTestFailed { .. } => "live, last connection test failed",
            Self::FailedLastTestPassed { .. } => "registration failed, last connection test passed",
            Self::Reachable(_) => "connection test passed",
            Self::Configured => "configured",
            Self::Failed(_) => "failed",
            Self::Unreachable(_) => "connection test failed",
            Self::Disabled => "disabled",
        }
    }
}

static MCP_EVIDENCE: OnceLock<Mutex<McpEvidence>> = OnceLock::new();

/// Lazily initialize and borrow the process-wide evidence owner.
fn evidence_store() -> &'static Mutex<McpEvidence> {
    MCP_EVIDENCE.get_or_init(|| Mutex::new(McpEvidence::default()))
}

/// Copy of the current evidence for one probe pass.
fn evidence_snapshot(store: &Mutex<McpEvidence>) -> McpEvidence {
    store
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

/// Replace the runtime ledger with the outcome of one discovery pass. A
/// whole-map replacement, so servers dropped from the config do not linger as
/// stale rows. Connection tests are untouched.
fn record_runtime(store: &Mutex<McpEvidence>, mut snapshot: BTreeMap<String, RuntimeEvidence>) {
    let mut evidence = store.lock().unwrap_or_else(|poison| poison.into_inner());
    let seq = evidence.next_seq();
    for entry in snapshot.values_mut() {
        entry.seq = seq;
    }
    evidence.runtime = snapshot;
}

/// Test one configured server from the Settings Test action and record the
/// outcome as connection evidence for that exact entry. Bounded by `timeout`.
/// Never registers tools: the running agent's registry is unchanged.
pub fn test_configured_server(name: &str, timeout: Duration) -> Result<McpProbeSummary> {
    let path = codescribe_core::mcp::default_mcp_config_path()?;
    test_configured_server_at(evidence_store(), &path, name, timeout)
}

/// Testable core of [`test_configured_server`] against an explicit config and
/// evidence owner.
fn test_configured_server_at(
    store: &Mutex<McpEvidence>,
    path: &Path,
    name: &str,
    timeout: Duration,
) -> Result<McpProbeSummary> {
    let config = McpConfigFile::load(path)?;
    let server = config
        .servers
        .get(name)
        .with_context(|| format!("MCP server \"{name}\" not found"))?
        .clone();
    let outcome = probe_server_config_blocking(server.clone(), timeout);
    let recorded = match &outcome {
        Ok(summary) => Ok(summary.clone()),
        Err(error) => Err(anyhow_root_cause(error)),
    };
    let mut evidence = store.lock().unwrap_or_else(|poison| poison.into_inner());
    let seq = evidence.next_seq();
    evidence.tests.insert(
        name.to_string(),
        TestEvidence {
            config: server,
            outcome: recorded,
            seq,
        },
    );
    drop(evidence);
    outcome
}

/// Innermost cause of an error, as the string shown to the user. The context
/// chain reads as boilerplate in a status row; the root cause ("command not
/// found", "expected value at line 1") is the actionable part.
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

/// Which status line a row is. Stable machine identity for the Settings layer,
/// which renders a localized label per facet; `label` stays the English text
/// for logs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStatusFacet {
    /// Core readiness verdict.
    Readiness,
    /// Assistive provider + credential access.
    Provider,
    /// Compiled-in native tool count.
    NativeTools,
    /// Settings roots vs native-tool roots.
    WorkspaceRoots,
    VibecraftedRuntime,
    AicxMcp,
    LoctreeMcp,
    PrviewIntegration,
    /// `mcp.json` itself: missing, empty, unreadable, or a parse note.
    McpConfig,
    /// One configured MCP server; `subject` carries its name.
    McpServer,
}

/// Machine state behind a row's value text. Each facet uses the subset that
/// applies; the Settings layer renders a localized sentence per state and
/// interpolates `count`, `subject`, and `detail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStatusState {
    /// Readiness verdict passed (`subject` = provider label, `count` = native tools).
    Ready,
    /// Provider access available (`subject` = provider label).
    AccessAvailable,
    /// Provider access missing (`subject` = provider label, `detail` = env key).
    AccessUnavailable,
    /// No native tools compiled in.
    NoNativeTools,
    /// Native tools present (`count`).
    Available,
    /// Settings roots match native-tool roots (`count` = roots).
    Synchronized,
    /// Settings roots differ from native-tool roots (`detail` = both lists).
    RootsMismatch,
    /// Optional server absent from `mcp.json`.
    NotConfigured,
    /// Discovery succeeded (`count` = tools, `subject` = server name when relevant).
    Live,
    /// Discovery failed (`detail` = root cause).
    Failed,
    Disabled,
    /// Configured but the agent has not run discovery yet.
    Configured,
    /// Last connection test answered (`count` = tools); the agent has not
    /// registered this server's tools.
    Reachable,
    /// Last connection test failed (`detail` = root cause); the agent has not
    /// registered this server either.
    Unreachable,
    /// The agent registered tools (`count`), and a connection test recorded
    /// after that registration failed (`detail` = root cause).
    LiveLastTestFailed,
    /// Registration failed (`detail` = root cause), and a connection test
    /// recorded after it answered (`count` = tools); nothing is registered.
    FailedLastTestPassed,
    /// No server is identified as this operator tool, but configured servers
    /// without identity evidence could be it (`detail` = their names).
    Unverified,
    /// `mcp.json` unreadable or its path unavailable (`detail` = cause).
    Error,
    /// `mcp.json` present with no servers.
    Empty,
    /// No `mcp.json` at all.
    Missing,
    /// Informational note (`detail`).
    Note,
}

/// One labelled status line in the Engine tab's "MCP Servers" section.
///
/// `label` / `value` are the English rendering used by logs and tests; the
/// Settings layer renders its own localized text from `facet`, `state`, and the
/// structured parts, never by parsing `value`.
pub struct McpStatusRow {
    pub label: String,
    pub value: String,
    pub tone: McpRowTone,
    pub facet: McpStatusFacet,
    pub state: McpStatusState,
    /// Tool or folder count behind the value, when the state carries one.
    pub count: Option<u32>,
    /// Provider label or server name behind the value, when there is one.
    pub subject: String,
    /// Error cause, env key, or free-form note behind the value.
    pub detail: String,
}

impl McpStatusRow {
    /// Row with no count, subject, or detail; callers fill the parts that apply.
    fn new(
        facet: McpStatusFacet,
        state: McpStatusState,
        label: impl Into<String>,
        value: impl Into<String>,
        tone: McpRowTone,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone,
            facet,
            state,
            count: None,
            subject: String::new(),
            detail: String::new(),
        }
    }
}

/// Honest snapshot of MCP config + runtime state for the Settings UI.
pub struct McpStatusReport {
    pub config_path_display: String,
    /// Whether the user has an `mcp.json` with at least one server defined.
    /// `false` means "no config yet" (missing file OR present-but-empty) — the
    /// onboarding readiness step surfaces the setup prompt instead of a status
    /// card. A present config that fails to load still counts as `true`: the
    /// file exists, the user configured something, so we show the concrete error
    /// rather than pretend nothing is there.
    configured: bool,
    rows: Vec<McpStatusRow>,
}

impl McpStatusReport {
    /// Status rows to render, in display order.
    pub fn summary_rows(&self) -> &[McpStatusRow] {
        &self.rows
    }

    /// `true` when an `mcp.json` exists with at least one server defined (or when
    /// a present config failed to load — the file is still there). `false` only
    /// when there is no config yet: missing file or present-but-no-servers.
    pub fn configured(&self) -> bool {
        self.configured
    }

    /// Build a report carrying exactly one row — the terminal states (no
    /// config, unreadable config, config with no servers) where per-server rows
    /// would be meaningless.
    fn single(
        config_path_display: String,
        configured: bool,
        label: &str,
        value: String,
        tone: McpRowTone,
        state: McpStatusState,
        detail: String,
    ) -> Self {
        Self {
            config_path_display,
            configured,
            rows: vec![McpStatusRow {
                detail,
                ..McpStatusRow::new(McpStatusFacet::McpConfig, state, label, value, tone)
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
                true,
                "Status:",
                format!("config path unavailable: {error}"),
                McpRowTone::Bad,
                McpStatusState::Error,
                error.to_string(),
            );
        }
    };
    probe_mcp_status_at(&path)
}

/// Testable core of [`probe_mcp_status`] against an explicit config path.
/// Emits one row per configured server, sorted by name, merging the recorded
/// evidence with the config's own enabled flag.
fn probe_mcp_status_at(path: &Path) -> McpStatusReport {
    probe_mcp_status_with(path, &evidence_snapshot(evidence_store()))
}

/// [`probe_mcp_status_at`] against an explicit evidence snapshot.
fn probe_mcp_status_with(path: &Path, evidence: &McpEvidence) -> McpStatusReport {
    let config_path_display = path.display().to_string();

    if !path.exists() {
        return McpStatusReport::single(
            config_path_display,
            false,
            "Status:",
            "no mcp.json (optional — MCP off)".to_string(),
            McpRowTone::Neutral,
            McpStatusState::Missing,
            String::new(),
        );
    }

    let config = match McpConfigFile::load(path) {
        Ok(config) => config,
        Err(error) => {
            let cause = anyhow_root_cause(&error);
            return McpStatusReport::single(
                config_path_display,
                true,
                "Config error:",
                cause.clone(),
                McpRowTone::Bad,
                McpStatusState::Error,
                cause,
            );
        }
    };

    if config.servers.is_empty() {
        return McpStatusReport::single(
            config_path_display,
            false,
            "Status:",
            "config present, no servers defined".to_string(),
            McpRowTone::Warn,
            McpStatusState::Empty,
            String::new(),
        );
    }

    let mut names: Vec<&String> = config.servers.keys().collect();
    names.sort();
    let mut rows = Vec::with_capacity(names.len());
    for name in names {
        let state = evidence.server_state(name, &config.servers[name]);
        let (value, tone, state, count, detail) = match state {
            ServerState::Live(count) => (
                format!("{count} tool(s)"),
                McpRowTone::Good,
                McpStatusState::Live,
                Some(count as u32),
                String::new(),
            ),
            ServerState::LiveLastTestFailed { tools, reason } => (
                format!("{tools} tool(s); last connection test failed: {reason}"),
                McpRowTone::Warn,
                McpStatusState::LiveLastTestFailed,
                Some(tools as u32),
                reason,
            ),
            ServerState::FailedLastTestPassed { reason, tools } => (
                format!(
                    "registration failed: {reason}; last connection test passed — {tools} tool(s)"
                ),
                McpRowTone::Warn,
                McpStatusState::FailedLastTestPassed,
                Some(tools as u32),
                reason,
            ),
            ServerState::Reachable(count) => (
                format!(
                    "connection test passed — {count} tool(s), not registered by the agent yet"
                ),
                McpRowTone::Warn,
                McpStatusState::Reachable,
                Some(count as u32),
                String::new(),
            ),
            ServerState::Failed(reason) => (
                format!("failed: {reason}"),
                McpRowTone::Bad,
                McpStatusState::Failed,
                None,
                reason,
            ),
            ServerState::Unreachable(reason) => (
                format!("connection test failed: {reason}"),
                McpRowTone::Bad,
                McpStatusState::Unreachable,
                None,
                reason,
            ),
            ServerState::Disabled => (
                "disabled".to_string(),
                McpRowTone::Neutral,
                McpStatusState::Disabled,
                None,
                String::new(),
            ),
            ServerState::Configured => (
                "configured (agent not started)".to_string(),
                McpRowTone::Warn,
                McpStatusState::Configured,
                None,
                String::new(),
            ),
        };
        rows.push(McpStatusRow {
            count,
            subject: name.clone(),
            detail,
            ..McpStatusRow::new(
                McpStatusFacet::McpServer,
                state,
                format!("{name}:"),
                value,
                tone,
            )
        });
    }

    McpStatusReport {
        config_path_display,
        configured: true,
        rows,
    }
}

/// One operator-tooling MCP server surfaced as an INFORMATIONAL readiness row.
///
/// These are Vetcoders operator surfaces (Vibecrafted / AICX / Loctree); an
/// end-user install will not have them. Per the C4 readiness-semantics decision
/// they are context only and NEVER gate `ready` — the core capability gate
/// (provider + key + native tools) is the sole arbiter of readiness. PRView is
/// handled separately by [`classify_prview`], also as optional context.
struct OperatorTool {
    facet: McpStatusFacet,
    label: &'static str,
    /// Canonical stdio server and binary name (`loctree-mcp`).
    canonical: &'static str,
    /// Product name a server may advertise or be named after (`loctree`).
    product: &'static str,
}

const OPERATOR_TOOLS: &[OperatorTool] = &[
    OperatorTool {
        facet: McpStatusFacet::VibecraftedRuntime,
        label: "Vibecrafted runtime:",
        canonical: "vibecrafted-mcp",
        product: "vibecrafted",
    },
    OperatorTool {
        facet: McpStatusFacet::AicxMcp,
        label: "AICX MCP:",
        canonical: "aicx-mcp",
        product: "aicx",
    },
    OperatorTool {
        facet: McpStatusFacet::LoctreeMcp,
        label: "Loctree MCP:",
        canonical: "loctree-mcp",
        product: "loctree",
    },
];

/// Transport words accepted after the product in a server name
/// (`loctree-http`, `aicx_mcp`). A closed list: `aicx-dragon` or
/// `my-loctree` are not claimed by name.
const NAME_TRANSPORT_SUFFIXES: &[&str] =
    &["mcp", "http", "https", "sse", "stdio", "remote", "local"];

/// Why a configured server is taken to be a given operator tool.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IdentityEvidence {
    /// The `mcp.json` key is the canonical name (`loctree-mcp`).
    CanonicalName,
    /// A stdio entry spawns the canonical binary (`…/loctree-mcp`).
    Command,
    /// The server advertised the product identity in its handshake.
    Advertised(String),
    /// The key is the product plus a transport word (`loctree-http`) and the
    /// server has not advertised a different identity.
    NameConvention,
}

impl IdentityEvidence {
    /// Plain-English reason, used in selection explanations and logs.
    fn describe(&self) -> String {
        match self {
            Self::CanonicalName => "canonical server name".to_string(),
            Self::Command => "canonical command".to_string(),
            Self::Advertised(identity) => format!("advertised identity \"{identity}\""),
            Self::NameConvention => "server name".to_string(),
        }
    }
}

impl OperatorTool {
    /// Whether an advertised `serverInfo.name` is this tool.
    fn is_identity(&self, identity: &str) -> bool {
        let identity = identity.trim().to_ascii_lowercase();
        identity == self.canonical || identity == self.product
    }

    /// Whether `name` is the product alone or product + separator + one of
    /// [`NAME_TRANSPORT_SUFFIXES`].
    fn follows_name_convention(&self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        let Some(rest) = name.strip_prefix(self.product) else {
            return false;
        };
        rest.is_empty()
            || rest
                .strip_prefix('-')
                .or_else(|| rest.strip_prefix('_'))
                .is_some_and(|suffix| NAME_TRANSPORT_SUFFIXES.contains(&suffix))
    }

    /// Evidence that the configured server `name` is this tool, or `None`.
    /// Explicit configuration (canonical key, canonical stdio command) holds on
    /// its own; otherwise an advertised identity decides, and the naming
    /// convention applies only while the server has advertised nothing.
    fn identify(
        &self,
        name: &str,
        config: &McpServerConfig,
        advertised: Option<&str>,
    ) -> Option<IdentityEvidence> {
        if name == self.canonical {
            return Some(IdentityEvidence::CanonicalName);
        }
        if config.url.is_none()
            && Path::new(config.command.trim())
                .file_name()
                .is_some_and(|binary| binary == self.canonical)
        {
            return Some(IdentityEvidence::Command);
        }
        match advertised {
            Some(identity) if self.is_identity(identity) => {
                Some(IdentityEvidence::Advertised(identity.to_string()))
            }
            Some(_) => None,
            None => self
                .follows_name_convention(name)
                .then_some(IdentityEvidence::NameConvention),
        }
    }
}

/// One configured server matched to an operator tool, with its state.
struct OperatorMatch<'a> {
    name: &'a str,
    evidence: IdentityEvidence,
    state: ServerState,
}

/// Whether any operator tool or PRView claims `name` by config or identity.
fn is_identified(name: &str, config: &McpServerConfig, evidence: &McpEvidence) -> bool {
    let advertised = evidence.advertised_for(name, config);
    advertised.is_some()
        || is_prview_name(name)
        || OPERATOR_TOOLS
            .iter()
            .any(|tool| tool.identify(name, config, None).is_some())
}

/// Core capability gate — the REAL ability of the agent to act. This is the only
/// input that decides `ready`: a usable sealed assistive lane with current
/// credential access (account access, API key, or a key-optional provider),
/// the native tool set, and exact agreement between
/// the persisted Settings roots and the roots resolved by native tools. Operator
/// tooling (MCP servers) is informational and never enters this verdict.
#[derive(Debug, Clone)]
pub struct CoreReadiness {
    /// Display name of the resolved assistive-lane provider.
    pub provider_label: String,
    /// Keychain/env account holding that provider's assistive key.
    pub key_env_key: String,
    /// Whether the sealed lane is usable and a request has credential access.
    pub provider_access_available: bool,
    /// Number of native (compiled-in) tools available to the agent.
    pub native_tool_count: usize,
    /// Roots rendered by Settings from fresh persisted config.
    pub configured_workspace_roots: Vec<String>,
    /// Roots the native workspace/file tools actually resolve.
    pub tool_workspace_roots: Vec<String>,
}

/// Probe the core capability gate from one immutable loader result: the sealed
/// assistive lane, its loader-owned availability verdict, and the count of
/// native tools, plus persisted/tool workspace-root parity. No secret, env, or
/// settings store is read here.
pub fn probe_core_readiness(runtime_settings: &RuntimeSettingsSnapshot) -> CoreReadiness {
    let assistive_lane = runtime_settings.llm_lanes().assistive();
    let configured_workspace_roots = configured_workspace_roots(runtime_settings);
    let tool_workspace_roots = super::workspace::configured_roots();
    assemble_core_readiness(
        assistive_lane.provider_display_name().to_string(),
        assistive_lane.credential().key_account().to_string(),
        assistive_lane.available() && assistive_lane.request_available(),
        configured_workspace_roots,
        tool_workspace_roots,
    )
}

fn configured_workspace_roots(runtime_settings: &RuntimeSettingsSnapshot) -> Vec<String> {
    let roots = normalize_agent_workspace_roots(
        runtime_settings
            .user_settings()
            .agent_workspace_roots
            .clone()
            .unwrap_or_default(),
    );
    if roots.is_empty() {
        vec![DEFAULT_AGENT_WORKSPACE_ROOT.to_string()]
    } else {
        roots
    }
}

/// Assemble a [`CoreReadiness`] from already-resolved inputs, counting native
/// tools by building a throwaway registry. Shared by the live probe and its
/// test variant so both count tools the same way.
fn assemble_core_readiness(
    provider_label: String,
    key_env_key: String,
    provider_access_available: bool,
    configured_workspace_roots: Vec<String>,
    tool_workspace_roots: Vec<String>,
) -> CoreReadiness {
    let mut registry = ToolRegistry::new();
    super::register_native_tools(&mut registry);
    let native_tool_count = registry.definitions().len();

    CoreReadiness {
        provider_label,
        key_env_key,
        provider_access_available,
        native_tool_count,
        configured_workspace_roots,
        tool_workspace_roots,
    }
}

/// Whether Settings and the native tools resolve exactly the same, non-empty
/// root list. Empty counts as a mismatch: an agent with no reachable workspace
/// is not ready, however consistent the two sides are about it.
fn workspace_roots_match(core: &CoreReadiness) -> bool {
    !core.configured_workspace_roots.is_empty()
        && core.configured_workspace_roots == core.tool_workspace_roots
}

/// Readiness verdict for the Agentic operating lane.
///
/// `ready` is decided SOLELY by the core capability gate ([`CoreReadiness`]:
/// assistive provider access available + native tools compiled in +
/// exact Settings/native-tool workspace-root parity).
/// The per-server MCP rows (Vibecrafted / AICX / Loctree / PRView) are
/// INFORMATIONAL context only — they can never flip `ready` — because they are
/// operator tooling an end-user install will not have. This is the C4
/// readiness-semantics decision (redesign from the earlier "all four required"
/// gate, which left a working agent stuck at NOT READY).
pub struct AgenticReadinessReport {
    pub config_path_display: String,
    ready: bool,
    rows: Vec<McpStatusRow>,
}

impl AgenticReadinessReport {
    /// Rows to render, in display order: the core-gate rows that decide
    /// `ready` first, then the informational operator-tooling rows.
    pub fn summary_rows(&self) -> &[McpStatusRow] {
        &self.rows
    }

    /// `true` only when the core capability gate passes: a configured assistive
    /// provider with request access, at least one native tool available, and
    /// matching non-empty Settings/native-tool workspace roots.
    /// Operator-tooling MCP rows are informational and never affect this verdict.
    pub fn is_ready(&self) -> bool {
        self.ready
    }
}

/// Classify one operator tool as an INFORMATIONAL row. Never blocking: a
/// missing/failed/disabled operator server does not affect agent readiness
/// (the core gate owns that).
///
/// Every configured server is checked with [`OperatorTool::identify`]. With
/// several matches the strongest state wins (live, connection test passed,
/// configured, failed, test failed, disabled), then explicit evidence over the
/// naming convention, then the name; `value` names the selection and the other
/// matches. With no match, servers that carry no identity evidence at all make
/// the row `Unverified` instead of claiming the tool is absent.
fn classify_operator_tool(
    tool: &OperatorTool,
    config: &McpConfigFile,
    evidence: &McpEvidence,
) -> McpStatusRow {
    let mut names: Vec<&String> = config.servers.keys().collect();
    names.sort();
    let mut matches: Vec<OperatorMatch<'_>> = names
        .iter()
        .filter_map(|name| {
            let server = &config.servers[*name];
            let identity = tool.identify(name, server, evidence.advertised_for(name, server))?;
            Some(OperatorMatch {
                name: name.as_str(),
                evidence: identity,
                state: evidence.server_state(name, server),
            })
        })
        .collect();
    matches.sort_by_key(|candidate| {
        (
            candidate.state.rank(),
            candidate.evidence == IdentityEvidence::NameConvention,
            candidate.name,
        )
    });

    let Some(selected) = matches.first() else {
        let unidentified: Vec<&str> = names
            .iter()
            .filter(|name| {
                let server = &config.servers[name.as_str()];
                server.enabled.unwrap_or(true) && !is_identified(name, server, evidence)
            })
            .map(|name| name.as_str())
            .collect();
        if unidentified.is_empty() {
            return McpStatusRow::new(
                tool.facet,
                McpStatusState::NotConfigured,
                tool.label,
                "not configured (optional)",
                McpRowTone::Neutral,
            );
        }
        let list = unidentified.join(", ");
        return McpStatusRow {
            detail: list.clone(),
            ..McpStatusRow::new(
                tool.facet,
                McpStatusState::Unverified,
                tool.label,
                format!("not detected (optional) — identity unknown for: {list}"),
                McpRowTone::Neutral,
            )
        };
    };

    let mut why = format!(
        "via \"{}\" ({})",
        selected.name,
        selected.evidence.describe()
    );
    if matches.len() > 1 {
        let others: Vec<String> = matches[1..]
            .iter()
            .map(|other| format!("{} ({})", other.name, other.state.describe()))
            .collect();
        why.push_str(&format!("; also matched: {}", others.join(", ")));
    }
    let (state, value, tone, count, detail) = match &selected.state {
        ServerState::Live(count) => (
            McpStatusState::Live,
            format!("ready — {count} tool(s) live {why}"),
            McpRowTone::Good,
            Some(*count as u32),
            String::new(),
        ),
        ServerState::LiveLastTestFailed { tools, reason } => (
            McpStatusState::LiveLastTestFailed,
            format!("{tools} tool(s) live; last connection test failed: {reason} {why}"),
            McpRowTone::Warn,
            Some(*tools as u32),
            reason.clone(),
        ),
        ServerState::FailedLastTestPassed { reason, tools } => (
            McpStatusState::FailedLastTestPassed,
            format!(
                "registration failed: {reason}; last connection test passed — {tools} tool(s) {why}"
            ),
            McpRowTone::Warn,
            Some(*tools as u32),
            reason.clone(),
        ),
        ServerState::Reachable(count) => (
            McpStatusState::Reachable,
            format!(
                "connection test passed — {count} tool(s), not registered by the agent yet {why}"
            ),
            McpRowTone::Warn,
            Some(*count as u32),
            String::new(),
        ),
        // Configured but discovery failed: surface the concrete reason (warn,
        // not blocking — the agent still works without this operator surface).
        ServerState::Failed(reason) => (
            McpStatusState::Failed,
            format!("failed: {reason} {why}"),
            McpRowTone::Warn,
            None,
            reason.clone(),
        ),
        ServerState::Unreachable(reason) => (
            McpStatusState::Unreachable,
            format!("connection test failed: {reason} {why}"),
            McpRowTone::Warn,
            None,
            reason.clone(),
        ),
        ServerState::Disabled => (
            McpStatusState::Disabled,
            format!("disabled {why}"),
            McpRowTone::Neutral,
            None,
            String::new(),
        ),
        ServerState::Configured => (
            McpStatusState::Configured,
            format!("configured — agent not started yet {why}"),
            McpRowTone::Warn,
            None,
            String::new(),
        ),
    };
    McpStatusRow {
        count,
        subject: selected.name.to_string(),
        detail,
        ..McpStatusRow::new(tool.facet, state, tool.label, value, tone)
    }
}

/// PRView servers are recognised by name; see [`classify_prview`].
fn is_prview_name(name: &str) -> bool {
    name.to_ascii_lowercase().contains("prview")
}

/// Classify PRView as an INFORMATIONAL row. Per the C4 decision PRView is
/// OPTIONAL, not required substrate: its absence is a neutral "not configured
/// (optional)" and never blocks readiness. The canonical wiring is a `prview`
/// MCP server (`{"command":"prview","args":["mcp"]}`); we also honour any server
/// whose name contains "prview" so a manually wired entry is recognised.
fn classify_prview(config: &McpConfigFile, evidence: &McpEvidence) -> McpStatusRow {
    let mut detected: Vec<(&String, &McpServerConfig)> = config
        .servers
        .iter()
        .filter(|(name, _)| is_prview_name(name))
        .collect();
    detected.sort_by_key(|(name, _)| name.as_str());
    let detected = detected.first().copied();
    let facet = McpStatusFacet::PrviewIntegration;
    let label = "PRView integration:";
    let (value, tone, state, count, detail) = match detected {
        // PRView keeps reporting registration alone; its last-test detail is
        // out of scope for this row.
        Some((name, cfg)) => match evidence.server_state(name, cfg) {
            ServerState::Live(count) | ServerState::LiveLastTestFailed { tools: count, .. } => (
                format!("ready — {count} tool(s) live (via \"{name}\")"),
                McpRowTone::Good,
                McpStatusState::Live,
                Some(count as u32),
                String::new(),
            ),
            ServerState::Reachable(count) => (
                format!(
                    "connection test passed — {count} tool(s), not registered by the agent yet (via \"{name}\")"
                ),
                McpRowTone::Warn,
                McpStatusState::Reachable,
                Some(count as u32),
                String::new(),
            ),
            ServerState::Failed(reason) | ServerState::FailedLastTestPassed { reason, .. } => (
                format!("failed: {reason}"),
                McpRowTone::Warn,
                McpStatusState::Failed,
                None,
                reason,
            ),
            ServerState::Unreachable(reason) => (
                format!("connection test failed: {reason}"),
                McpRowTone::Warn,
                McpStatusState::Unreachable,
                None,
                reason,
            ),
            ServerState::Disabled => (
                "disabled".to_string(),
                McpRowTone::Neutral,
                McpStatusState::Disabled,
                None,
                String::new(),
            ),
            ServerState::Configured => (
                format!("configured — agent not started yet (via \"{name}\")"),
                McpRowTone::Warn,
                McpStatusState::Configured,
                None,
                String::new(),
            ),
        },
        None => (
            "not configured (optional)".to_string(),
            McpRowTone::Neutral,
            McpStatusState::NotConfigured,
            None,
            String::new(),
        ),
    };
    McpStatusRow {
        count,
        subject: detected.map(|(name, _)| name.clone()).unwrap_or_default(),
        detail,
        ..McpStatusRow::new(facet, state, label, value, tone)
    }
}

/// Agentic-lane readiness probe: `ready` is the core capability gate (provider +
/// key + native tools); the MCP rows are informational context. See
/// [`AgenticReadinessReport`] for the semantics decision.
pub fn probe_agentic_readiness(
    runtime_settings: &RuntimeSettingsSnapshot,
) -> AgenticReadinessReport {
    let core = probe_core_readiness(runtime_settings);
    let path = match codescribe_core::mcp::default_mcp_config_path() {
        Ok(path) => path,
        Err(error) => {
            // MCP is optional, so a missing config path is informational — the
            // core gate still decides readiness.
            return assemble_readiness(
                "unavailable".to_string(),
                core,
                McpConfigFile {
                    servers: Default::default(),
                },
                Some(format!("config path unavailable: {error}")),
                &evidence_snapshot(evidence_store()),
            );
        }
    };
    probe_agentic_readiness_at(&path, core)
}

/// Testable core of [`probe_agentic_readiness`]: an explicit config path plus
/// an explicit core verdict, so readiness tests stay free of process-env and
/// Keychain coupling. A missing or unparseable config yields an empty server
/// set (plus a warn note), never an error.
fn probe_agentic_readiness_at(path: &Path, core: CoreReadiness) -> AgenticReadinessReport {
    probe_agentic_readiness_with(path, core, &evidence_snapshot(evidence_store()))
}

/// [`probe_agentic_readiness_at`] against an explicit evidence snapshot.
fn probe_agentic_readiness_with(
    path: &Path,
    core: CoreReadiness,
    evidence: &McpEvidence,
) -> AgenticReadinessReport {
    let config_path_display = path.display().to_string();

    let (config, config_note) = if !path.exists() {
        (
            McpConfigFile {
                servers: Default::default(),
            },
            None,
        )
    } else {
        match McpConfigFile::load(path) {
            Ok(config) => (config, None),
            // A broken mcp.json no longer blocks readiness (MCP is optional); it is
            // surfaced as an informational warn row instead of a hard not-ready.
            Err(error) => (
                McpConfigFile {
                    servers: Default::default(),
                },
                Some(format!(
                    "mcp.json parse error (optional): {}",
                    anyhow_root_cause(&error)
                )),
            ),
        }
    };

    assemble_readiness(config_path_display, core, config, config_note, evidence)
}

/// Assemble the readiness report: the core-gate verdict + provider + native-tools
/// rows (which decide `ready`), followed by the informational operator-tooling
/// rows (which never do).
fn assemble_readiness(
    config_path_display: String,
    core: CoreReadiness,
    config: McpConfigFile,
    config_note: Option<String>,
    evidence: &McpEvidence,
) -> AgenticReadinessReport {
    let tools_present = core.native_tool_count > 0;
    let roots_match = workspace_roots_match(&core);
    let ready = core.provider_access_available && tools_present && roots_match;

    // ---- Core capability gate rows (these decide `ready`). ----
    let verdict = if ready {
        McpStatusRow {
            count: Some(core.native_tool_count as u32),
            subject: core.provider_label.clone(),
            ..McpStatusRow::new(
                McpStatusFacet::Readiness,
                McpStatusState::Ready,
                "Agentic readiness:",
                format!(
                    "ready — {} configured, access available, {} native tool(s)",
                    core.provider_label, core.native_tool_count
                ),
                McpRowTone::Good,
            )
        }
    } else {
        let (reason, state) = if !core.provider_access_available {
            (
                format!(
                    "assistive provider access unavailable (sign in or set {})",
                    core.key_env_key
                ),
                McpStatusState::AccessUnavailable,
            )
        } else if !tools_present {
            (
                "no native tools available".to_string(),
                McpStatusState::NoNativeTools,
            )
        } else {
            (
                "workspace roots differ between Settings and native tools".to_string(),
                McpStatusState::RootsMismatch,
            )
        };
        McpStatusRow {
            subject: core.provider_label.clone(),
            detail: core.key_env_key.clone(),
            ..McpStatusRow::new(
                McpStatusFacet::Readiness,
                state,
                "Agentic readiness:",
                format!("not ready — {reason}"),
                McpRowTone::Bad,
            )
        }
    };

    let provider_row = McpStatusRow {
        subject: core.provider_label.clone(),
        detail: core.key_env_key.clone(),
        ..McpStatusRow::new(
            McpStatusFacet::Provider,
            if core.provider_access_available {
                McpStatusState::AccessAvailable
            } else {
                McpStatusState::AccessUnavailable
            },
            "Provider:",
            if core.provider_access_available {
                format!("{} — access available", core.provider_label)
            } else {
                format!(
                    "{} — access unavailable (sign in or set {})",
                    core.provider_label, core.key_env_key
                )
            },
            if core.provider_access_available {
                McpRowTone::Good
            } else {
                McpRowTone::Bad
            },
        )
    };

    let tools_row = McpStatusRow {
        count: Some(core.native_tool_count as u32),
        ..McpStatusRow::new(
            McpStatusFacet::NativeTools,
            if tools_present {
                McpStatusState::Available
            } else {
                McpStatusState::NoNativeTools
            },
            "Native tools:",
            format!("{} tool(s) available", core.native_tool_count),
            if tools_present {
                McpRowTone::Good
            } else {
                McpRowTone::Bad
            },
        )
    };

    let roots_detail = format!(
        "Settings={:?}, native tools={:?}",
        core.configured_workspace_roots, core.tool_workspace_roots
    );
    let roots_row = McpStatusRow {
        count: Some(core.configured_workspace_roots.len() as u32),
        detail: roots_detail.clone(),
        ..McpStatusRow::new(
            McpStatusFacet::WorkspaceRoots,
            if roots_match {
                McpStatusState::Synchronized
            } else {
                McpStatusState::RootsMismatch
            },
            "Workspace roots:",
            if roots_match {
                format!(
                    "{} configured — native tools synchronized",
                    core.configured_workspace_roots.len()
                )
            } else {
                format!("mismatch — {roots_detail}")
            },
            if roots_match {
                McpRowTone::Good
            } else {
                McpRowTone::Bad
            },
        )
    };

    let mut rows = Vec::with_capacity(OPERATOR_TOOLS.len() + 6);
    rows.push(verdict);
    rows.push(provider_row);
    rows.push(tools_row);
    rows.push(roots_row);

    // ---- Informational operator-tooling rows (never gate `ready`). ----
    if let Some(note) = config_note {
        rows.push(McpStatusRow {
            detail: note.clone(),
            ..McpStatusRow::new(
                McpStatusFacet::McpConfig,
                McpStatusState::Note,
                "MCP config:",
                note,
                McpRowTone::Warn,
            )
        });
    }
    for tool in OPERATOR_TOOLS {
        rows.push(classify_operator_tool(tool, &config, evidence));
    }
    rows.push(classify_prview(&config, evidence));

    AgenticReadinessReport {
        config_path_display,
        ready,
        rows,
    }
}

/// Register every MCP tool from the user's `mcp.json` into the agent registry.
///
/// MCP is optional, so this never fails the caller: an unavailable config path
/// or a failed registration is logged and skipped. Also populates the runtime
/// discovery cache read back by the Settings Engine tab.
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

/// Register tools from an explicit config path and return how many landed.
/// A missing file is not an error — it yields zero, since MCP is opt-in.
pub(crate) fn register_mcp_tools_from_config_path(
    registry: &mut ToolRegistry,
    path: &Path,
) -> Result<usize> {
    register_mcp_tools_into(registry, path, evidence_store())
}

/// [`register_mcp_tools_from_config_path`] recording discovery into an
/// explicit evidence owner.
fn register_mcp_tools_into(
    registry: &mut ToolRegistry,
    path: &Path,
    store: &Mutex<McpEvidence>,
) -> Result<usize> {
    let Some(config) = McpConfigFile::load_optional(path)? else {
        return Ok(0);
    };
    register_mcp_tools_from_config(registry, config, store)
}

/// Run discovery, then register each discovered tool behind a closure that
/// dispatches to its own server. Per-tool failures (unsafe name, duplicate
/// registration) are warned and skipped so one bad tool cannot cost the rest;
/// the count reflects what actually registered.
fn register_mcp_tools_from_config(
    registry: &mut ToolRegistry,
    config: McpConfigFile,
    store: &Mutex<McpEvidence>,
) -> Result<usize> {
    let (discovered, runtime) = discover_mcp_tools_blocking(config)?;
    record_runtime(store, runtime);
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
            input_schema: public_input_schema(
                &discovered_tool.server_name,
                &original_tool_name,
                &discovered_tool.tool.input_schema,
            ),
        };

        let (policy, validator) = execution_policy(&server_name, &original_tool_name);
        let register_result = registry.register_with_policy(
            definition,
            Box::new(move |input| {
                let client = McpClient::new(client_config.clone());
                let tool_name = original_tool_name.clone();
                let server = server_name.clone();
                Box::pin(async move {
                    let input = match prepare_upstream_input(&server, &tool_name, input) {
                        Ok(input) => input,
                        Err(error) => return vec![ToolResultContent::Error(error.to_string())],
                    };
                    match client.call_tool(&tool_name, input).await {
                        Ok(output) => output,
                        Err(error) => vec![ToolResultContent::Error(format!(
                            "MCP tool '{server}/{tool_name}' failed: {error}"
                        ))],
                    }
                })
            }),
            policy,
            validator,
        );

        if let Err(error) = register_result {
            warn!("Skipping duplicate MCP tool registration: {error}");
            continue;
        }

        registered += 1;
    }

    Ok(registered)
}

/// The schema the model sees, which may differ from the server's own.
///
/// Only Desktop Commander's `start_process` is rewritten: a required `cwd` is
/// added so the working directory becomes an explicit, validatable argument
/// instead of being smuggled inside the command string.
/// [`prepare_upstream_input`] folds it back before the call goes out.
fn public_input_schema(
    server: &str,
    tool: &str,
    upstream: &serde_json::Value,
) -> serde_json::Value {
    let mut schema = upstream.clone();
    if !is_desktop_commander_server(server) || tool != "start_process" {
        return schema;
    }
    let Some(object) = schema.as_object_mut() else {
        return schema;
    };
    let properties = object
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(properties) = properties.as_object_mut() {
        properties.insert(
            "cwd".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Absolute working directory inside a configured Codescribe Agent workspace root"
            }),
        );
    }
    let required = object
        .entry("required")
        .or_insert_with(|| serde_json::json!([]));
    if let Some(required) = required.as_array_mut()
        && !required.iter().any(|value| value == "cwd")
    {
        required.push(serde_json::json!("cwd"));
    }
    schema
}

/// Translate validated tool input into what the server actually expects.
///
/// The inverse of [`public_input_schema`]: for Desktop Commander's
/// `start_process`, the Codescribe-only `cwd` field is removed and folded into
/// the command as a quoted `cd`, so policy metadata never travels upstream.
/// Every other tool passes through untouched.
fn prepare_upstream_input(
    server: &str,
    tool: &str,
    mut input: serde_json::Value,
) -> Result<serde_json::Value> {
    if !is_desktop_commander_server(server) || tool != "start_process" {
        return Ok(input);
    }
    let cwd = required_string(&input, &["cwd"])?.to_string();
    let command = required_string(&input, &["command"])?.to_string();
    let object = input
        .as_object_mut()
        .context("Desktop Commander start_process input must be an object")?;
    object.remove("cwd");
    object.insert(
        "command".to_string(),
        serde_json::Value::String(format!("cd -- {} && {{ {}; }}", shell_quote(&cwd), command)),
    );
    Ok(input)
}

/// Wrap a value in single quotes for POSIX shell, escaping embedded quotes, so
/// a validated path cannot break out of the `cd` that carries it.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Whether this server is Desktop Commander, which gets the hardened policy
/// profile. Case-insensitive: the server name comes from user-written config.
fn is_desktop_commander_server(server_name: &str) -> bool {
    server_name.eq_ignore_ascii_case("desktop-commander")
}

/// Risk classification and optional input validator for one discovered tool.
///
/// Unknown servers get `ToolRisk::Unknown` with no validator — the registry's
/// own decision step turns Unknown into "require approval", so an unclassified
/// tool is never a silent allow. Desktop Commander is classified per tool:
/// reads run freely, mutations and process control require approval, and
/// anything unrecognised stays Unknown.
fn execution_policy(
    server_name: &str,
    upstream_tool: &str,
) -> (ToolExecutionPolicy, Option<ToolInputValidator>) {
    let origin = ToolOrigin::Mcp {
        server: server_name.to_string(),
        upstream_tool: upstream_tool.to_string(),
    };
    if !is_desktop_commander_server(server_name) {
        return (
            ToolExecutionPolicy {
                origin,
                risk: ToolRisk::Unknown,
                requires_approval: false,
            },
            None,
        );
    }

    let (risk, requires_approval) = match upstream_tool {
        "get_config"
        | "get_file_info"
        | "list_directory"
        | "list_processes"
        | "list_sessions"
        | "list_searches"
        | "get_more_search_results"
        | "read_process_output"
        | "get_usage_stats"
        | "get_recent_tool_calls"
        | "read_file"
        | "read_multiple_files"
        | "start_search"
        | "stop_search"
        | "get_prompts" => (ToolRisk::ReadOnly, false),
        "write_file" | "write_pdf" | "edit_block" | "move_file" | "create_directory" => {
            (ToolRisk::Mutating, true)
        }
        "start_process"
        | "interact_with_process"
        | "force_terminate"
        | "kill_process"
        | "set_config_value" => (ToolRisk::ProcessControl, true),
        _ => (ToolRisk::Unknown, false),
    };
    let validator = desktop_commander_validator(upstream_tool);
    (
        ToolExecutionPolicy {
            origin,
            risk,
            requires_approval,
        },
        validator,
    )
}

/// Build the per-call validator for a Desktop Commander tool.
///
/// The validator runs before every call and produces the approval preview. It
/// is the enforcement point for three rules: every path argument must resolve
/// inside a configured workspace root, URL reads are denied outright, and
/// Desktop Commander may not rewrite its own security-policy keys (matched on
/// a normalized key, so `security.allowedDirectories[0]` cannot slip through).
/// Returning `Err` aborts the call.
fn desktop_commander_validator(upstream_tool: &str) -> Option<ToolInputValidator> {
    let tool = upstream_tool.to_string();
    Some(Arc::new(move |input| {
        let roots = path_policy::workspace_roots();
        match tool.as_str() {
            "get_file_info" | "list_directory" | "read_file" | "start_search" => {
                if input
                    .get("isUrl")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    bail!("Desktop Commander URL reads are denied by Codescribe policy");
                }
                let path = required_string(input, &["path"])?;
                let canonical = path_policy::validate_existing(path, &roots)?;
                Ok(ToolCallPreview {
                    summary: format!("{tool} inside the configured workspace"),
                    paths: vec![canonical.display().to_string()],
                    ..Default::default()
                })
            }
            "read_multiple_files" => {
                let paths = input
                    .get("paths")
                    .and_then(serde_json::Value::as_array)
                    .context("Missing required array field 'paths'")?;
                if paths.is_empty() {
                    bail!("Desktop Commander read_multiple_files requires at least one path");
                }
                let paths = paths
                    .iter()
                    .map(|path| {
                        let path = path
                            .as_str()
                            .context("read_multiple_files paths must be strings")?;
                        path_policy::validate_existing(path, &roots)
                            .map(|path| path.display().to_string())
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(ToolCallPreview {
                    summary: "Read multiple files inside the configured workspace".to_string(),
                    paths,
                    ..Default::default()
                })
            }
            "write_file" | "write_pdf" | "create_directory" => {
                let path = required_string(input, &["path"])?;
                let target = path_policy::validate_new_target(path, &roots)?;
                Ok(ToolCallPreview {
                    summary: format!("{tool} inside the configured workspace"),
                    paths: vec![target.display().to_string()],
                    ..Default::default()
                })
            }
            "edit_block" => {
                let path = required_string(input, &["file_path", "path"])?;
                let canonical = path_policy::validate_existing(path, &roots)?;
                Ok(ToolCallPreview {
                    summary: "Edit an existing workspace file".to_string(),
                    paths: vec![canonical.display().to_string()],
                    ..Default::default()
                })
            }
            "move_file" => {
                let source = required_string(input, &["source", "source_path"])?;
                let destination = required_string(input, &["destination", "destination_path"])?;
                let source = path_policy::validate_existing(source, &roots)?;
                let destination = path_policy::validate_new_target(destination, &roots)?;
                Ok(ToolCallPreview {
                    summary: "Move a file inside the configured workspace".to_string(),
                    paths: vec![
                        source.display().to_string(),
                        destination.display().to_string(),
                    ],
                    ..Default::default()
                })
            }
            "start_process" => {
                let command = required_string(input, &["command"])?;
                let cwd = required_string(input, &["cwd", "path"])?;
                let cwd = path_policy::validate_terminal(command, cwd, &roots)?;
                Ok(ToolCallPreview {
                    summary: "Start a process inside the configured workspace".to_string(),
                    command: Some(redact_command_for_approval(command)),
                    cwd: Some(cwd.display().to_string()),
                    ..Default::default()
                })
            }
            "set_config_value" => {
                let key = required_string(input, &["key"])?;
                let normalized = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if ["alloweddirectories", "blockedcommands", "codescribepolicy"]
                    .iter()
                    .any(|protected| normalized.contains(protected))
                {
                    bail!("Desktop Commander may not change security-policy key '{key}'");
                }
                Ok(ToolCallPreview {
                    summary: format!("Change Desktop Commander setting '{key}'"),
                    ..Default::default()
                })
            }
            "interact_with_process" | "force_terminate" | "kill_process" => Ok(ToolCallPreview {
                summary: format!("Desktop Commander process control: {tool}"),
                ..Default::default()
            }),
            _ => Ok(ToolCallPreview {
                summary: format!("Desktop Commander tool: {tool}"),
                ..Default::default()
            }),
        }
    }))
}

/// Redact secrets from a command line before it is shown in an approval
/// prompt. Handles both shapes: `KEY=value` inline assignments, and a flag
/// whose secret sits in the following token(s) — `Authorization`/`Cookie` take
/// two, everything else one. Display only; the command sent upstream is
/// unchanged.
fn redact_command_for_approval(command: &str) -> String {
    /// Substring markers that trigger redaction of `KEY=value` tokens or the
    /// following flag argument(s) in approval previews.
    const SENSITIVE_KEYS: &[&str] = &[
        "token",
        "password",
        "passwd",
        "secret",
        "api_key",
        "apikey",
        "authorization",
        "cookie",
    ];
    let mut redact_remaining = 0usize;
    command
        .split_whitespace()
        .map(|token| {
            if redact_remaining > 0 {
                redact_remaining -= 1;
                return "[REDACTED]".to_string();
            }
            let trimmed = token.trim_matches(|character| matches!(character, '\'' | '"'));
            let normalized = trimmed.to_ascii_lowercase();
            if let Some((key, _)) = trimmed.split_once('=')
                && SENSITIVE_KEYS
                    .iter()
                    .any(|sensitive| key.to_ascii_lowercase().contains(sensitive))
            {
                return format!("{key}=[REDACTED]");
            }
            if SENSITIVE_KEYS
                .iter()
                .any(|sensitive| normalized.contains(sensitive))
            {
                redact_remaining =
                    if normalized.contains("authorization") || normalized.contains("cookie") {
                        2
                    } else {
                        1
                    };
            }
            token.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// First non-blank string among the given keys, or an error naming all of
/// them. The key list absorbs upstream naming drift (`file_path` vs `path`,
/// `source` vs `source_path`) without duplicating the validator arms.
fn required_string<'a>(input: &'a serde_json::Value, keys: &[&str]) -> Result<&'a str> {
    for key in keys {
        if let Some(value) = input.get(key).and_then(serde_json::Value::as_str)
            && !value.trim().is_empty()
        {
            return Ok(value);
        }
    }
    bail!(
        "Missing required string field (expected one of: {})",
        keys.join(", ")
    )
}

/// One tool found on one server, carrying the config needed to call it later.
/// The server config is cloned per tool because each registered closure owns
/// its own client.
#[derive(Debug)]
struct DiscoveredMcpTool {
    server_name: String,
    server_config: McpServerConfig,
    tool: McpTool,
}

/// Probe every configured server and return the tools they expose.
///
/// Synchronous by contract — it is reached from the sync registration path —
/// so it runs its own current-thread runtime on a dedicated thread; see the
/// inline P2.4 note for why that is the correct choice rather than an
/// oversight. Servers are probed in parallel and in isolation: a failure is
/// recorded against that server alone and never propagates, and disabled
/// servers are still recorded so the UI can tell "off" from "missing".
fn discover_mcp_tools_blocking(
    config: McpConfigFile,
) -> Result<(Vec<DiscoveredMcpTool>, BTreeMap<String, RuntimeEvidence>)> {
    // P2.4 DEFERRED (cross-cut, owned by the runtime/bin group):
    // This spawns a std::thread and builds a fresh current_thread runtime to run
    // the MCP discovery handshake, which bypasses the intentional 4-worker cap of
    // the main multi-threaded runtime (bin/codescribe.rs). The pattern is kept
    // deliberately because this fn is a SYNC blocking call reached from
    // `register` → `register_all_tools` → `initialize_agent_runtime`, which may
    // itself run inside the main tokio runtime (agent-send path). Calling
    // `Runtime::block_on` directly from within a running runtime panics, and the
    // alternative — `Handle::current().block_on` — also panics when no reactor is
    // current (e.g. the test/CLI call sites that drive this synchronously). The
    // clean fix is to reuse a startup-cached `tokio::runtime::Handle` from the
    // main runtime (the same cached-Handle pattern noted in
    // app/controller/mod.rs::request_permission_runtime_reconcile and
    // ui/voice_chat/handlers/connectors.rs), which requires a `OnceLock<Handle>`
    // populated in bin/codescribe.rs — outside this file's single-ownership
    // domain. Until that cache exists, the dedicated thread + current_thread
    // runtime is the correct defensive choice (no runtime-nesting panic) and the
    // join() makes the discovery cost bounded and one-shot per agent-runtime init.
    thread::spawn(
        move || -> Result<(Vec<DiscoveredMcpTool>, BTreeMap<String, RuntimeEvidence>)> {
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
                // Probe every enabled server in PARALLEL and in ISOLATION: one dead
                // or hung server costs at most its own initialize/request timeout
                // and can never veto the other servers' tools — the session starts
                // degraded (that server absent, WARN in the log), never dead.
                let probes =
                    servers
                        .into_iter()
                        .map(|(server_name, server_config, enabled)| async move {
                            if !enabled {
                                return (server_name, server_config, None);
                            }
                            let client = McpClient::new(server_config.clone());
                            let outcome = client.probe().await;
                            (server_name, server_config, Some(outcome))
                        });
                let results = futures_util::future::join_all(probes).await;

                let mut discovered = Vec::new();
                let mut status: BTreeMap<String, RuntimeEvidence> = BTreeMap::new();
                for (server_name, server_config, outcome) in results {
                    let (outcome, advertised) = match outcome {
                        None => (ServerRuntime::Disabled, None),
                        Some(Ok(probe)) => {
                            let advertised = probe.handshake.server_name();
                            let count = probe.tools.len();
                            for tool in probe.tools {
                                discovered.push(DiscoveredMcpTool {
                                    server_name: server_name.clone(),
                                    server_config: server_config.clone(),
                                    tool,
                                });
                            }
                            (ServerRuntime::Tools(count), advertised)
                        }
                        Some(Err(error)) => {
                            // Concrete root cause (spawn failure, command not found,
                            // parse error, timeout, …) — surfaced to logs AND the UI.
                            let reason = anyhow_root_cause(&error);
                            warn!("MCP server '{server_name}' discovery failed: {reason}");
                            (ServerRuntime::Failed(reason), None)
                        }
                    };
                    status.insert(
                        server_name,
                        RuntimeEvidence {
                            config: server_config,
                            outcome,
                            advertised,
                            // Stamped by `record_runtime` under the store lock.
                            seq: 0,
                        },
                    );
                }
                (discovered, status)
            });

            Ok((discovered, status))
        },
    )
    .join()
    .map_err(|_| anyhow::anyhow!("MCP discovery thread panicked"))?
}

/// Compose the `mcp__<server>__<tool>` name the model calls. Both parts are
/// validated first, so a hostile or malformed name cannot forge a different
/// tool identity through the separator.
fn public_tool_name(server_name: &str, tool_name: &str) -> Result<String> {
    validate_name_part("server", server_name)?;
    validate_name_part("tool", tool_name)?;
    Ok(format!("mcp__{server_name}__{tool_name}"))
}

/// Require a non-empty name of ASCII alphanumerics, `_`, or `-`. `kind` only
/// labels the error message ("server" / "tool").
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

/// Unit tests for Desktop Commander policy, MCP probe honesty, registration,
/// and C4 agentic readiness (core gate vs optional operator tooling).
#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{
        McpRowTone, desktop_commander_validator, execution_policy, prepare_upstream_input,
        probe_agentic_readiness_at, probe_mcp_status_at, public_input_schema, public_tool_name,
        redact_command_for_approval, register_mcp_tools_from_config_path,
    };
    use codescribe_core::agent::{ToolRegistry, ToolResultContent, ToolRisk};
    use serde_json::json;

    /// Pin the Desktop Commander 0.26 tool surface: 26 tools, 15 read-only, 10
    /// approval-gated, and one Unknown (`give_feedback_to_desktop_commander`).
    #[test]
    fn desktop_commander_026_profile_classifies_all_26_tools() {
        /// Canonical Desktop Commander 0.26 tool name census used by the profile
        /// classification pin.
        const TOOLS: &[&str] = &[
            "create_directory",
            "edit_block",
            "force_terminate",
            "get_config",
            "get_file_info",
            "get_more_search_results",
            "get_prompts",
            "get_recent_tool_calls",
            "get_usage_stats",
            "give_feedback_to_desktop_commander",
            "interact_with_process",
            "kill_process",
            "list_directory",
            "list_processes",
            "list_searches",
            "list_sessions",
            "move_file",
            "read_file",
            "read_multiple_files",
            "read_process_output",
            "set_config_value",
            "start_process",
            "start_search",
            "stop_search",
            "write_file",
            "write_pdf",
        ];
        let policies = TOOLS
            .iter()
            .map(|tool| (*tool, execution_policy("desktop-commander", tool).0))
            .collect::<Vec<_>>();
        assert_eq!(policies.len(), 26);
        assert_eq!(
            policies
                .iter()
                .filter(|(_, policy)| policy.risk == ToolRisk::ReadOnly)
                .count(),
            15
        );
        assert_eq!(
            policies
                .iter()
                .filter(|(_, policy)| policy.requires_approval)
                .count(),
            10
        );
        assert_eq!(
            policies
                .iter()
                .filter(|(_, policy)| policy.risk == ToolRisk::Unknown)
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            vec!["give_feedback_to_desktop_commander"]
        );
    }

    /// Unclassified Desktop Commander tools stay `ToolRisk::Unknown` so registry
    /// `decide()` can require approval without hard-denying.
    #[test]
    fn unknown_desktop_commander_tool_carries_unknown_risk_and_gates_at_decide() {
        // Unclassified upstream tools keep Unknown risk; the registry's decide()
        // turns that into RequireApproval (never a silent allow, never a hard
        // deny — Destructive is the only hard deny).
        let (policy, _) = execution_policy("desktop-commander", "future_unclassified_tool");
        assert_eq!(policy.risk, ToolRisk::Unknown);
        assert!(!policy.requires_approval);
    }

    /// Approval previews redact secret-like tokens and the Desktop Commander
    /// validator rejects security-policy key aliases on `set_config_value`.
    #[test]
    fn desktop_commander_terminal_preview_redacts_secrets_and_blocks_policy_key_aliases() {
        assert_eq!(
            redact_command_for_approval("TOKEN=hunter2 curl --password swordfish example.test"),
            "TOKEN=[REDACTED] curl --password [REDACTED] example.test"
        );
        assert_eq!(
            redact_command_for_approval(
                "curl -H 'Authorization: Bearer hunter2' https://example.test"
            ),
            "curl -H 'Authorization: [REDACTED] [REDACTED] https://example.test"
        );
        let validator = desktop_commander_validator("set_config_value").expect("desktop validator");
        assert!(
            validator(&json!({"key": "security.allowedDirectories[0]", "value": "/tmp"})).is_err()
        );
    }

    /// Codescribe policy metadata and cwd are rewritten for preview/path safety
    /// and must never appear in the payload sent to the MCP server.
    #[test]
    fn desktop_commander_policy_metadata_is_not_sent_upstream() {
        let upstream = prepare_upstream_input(
            "Desktop-Commander",
            "start_process",
            json!({
                "command": "printf ok",
                "cwd": "/workspace/project",
                "timeout_ms": 1_000
            }),
        )
        .expect("prepare Desktop Commander call");
        assert!(upstream.get("cwd").is_none());
        assert_eq!(upstream["timeout_ms"], json!(1_000));
        assert_eq!(
            upstream["command"],
            json!("cd -- '/workspace/project' && { printf ok; }")
        );
        assert!(upstream.get("codescribePolicy").is_none());

        let (policy, _) = execution_policy("Desktop-Commander", "list_directory");
        assert_eq!(policy.risk, ToolRisk::ReadOnly);
    }

    /// Porkbun-like nested email lookaround stays on the registry/upstream
    /// schema. OpenAI adaptation is a provider-copy concern; MCP
    /// `validateToolInput` still sees the original pattern.
    #[test]
    fn porkbun_lookaround_schema_stays_on_the_mcp_registry_boundary() {
        let upstream = crate::agent::openai_schema::porkbun_update_contacts_input_schema();
        let public = public_input_schema("porkbun", "update_contacts", &upstream);
        assert_eq!(public, upstream);
        for pointer in [
            "/properties/contact/properties/email/pattern",
            "/properties/contacts/properties/registrant/properties/email/pattern",
            "/properties/contacts/properties/admin/properties/email/pattern",
            "/properties/contacts/properties/tech/properties/email/pattern",
            "/properties/contacts/properties/billing/properties/email/pattern",
        ] {
            assert_eq!(
                public.pointer(pointer).and_then(serde_json::Value::as_str),
                Some(crate::agent::openai_schema::ZOD_EMAIL_LOOKAROUND_PATTERN),
                "{pointer}"
            );
        }

        let arguments = json!({
            "domain": "example.com",
            "contact": {
                "firstName": "Ada",
                "country": "US",
                "email": ".u@example.com"
            }
        });
        let prepared = prepare_upstream_input("porkbun", "update_contacts", arguments.clone())
            .expect("non-Desktop-Commander input is forwarded untouched");
        assert_eq!(prepared, arguments);
        assert_eq!(
            prepared["contact"]["email"],
            json!(".u@example.com"),
            "Codescribe must not strip or rewrite MCP arguments; upstream Zod still rejects leading-dot and double-dot emails"
        );
    }

    /// Missing `mcp.json` is a single neutral/optional row — never a hard error —
    /// and reports as not configured for onboarding.
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
        // No config yet → the onboarding step must offer the setup prompt.
        assert!(!report.configured());
    }

    /// A present-but-unparseable `mcp.json` surfaces a concrete Bad config error
    /// and still counts as configured (no setup prompt).
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
        // File exists (user configured something) → show the error, not the
        // onboarding prompt.
        assert!(report.configured());
    }

    /// Present config with an empty `mcpServers` map warns and still counts as
    /// not configured so onboarding can offer setup.
    #[test]
    fn probe_reports_present_config_with_no_servers() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        fs::write(&path, json!({ "mcpServers": {} }).to_string()).expect("write config");
        let report = probe_mcp_status_at(&path);
        let rows = report.summary_rows();
        assert_eq!(rows[0].tone, McpRowTone::Warn);
        assert!(rows[0].value.contains("no servers"));
        // Present but empty config counts as "not configured yet" → prompt.
        assert!(!report.configured());
    }

    /// Configured servers appear as Warn rows before runtime discovery, with
    /// "agent not started" when the cache has no handshake yet.
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
        // A config with at least one server → configured; no setup prompt.
        assert!(report.configured());
    }

    /// End-to-end: register tools from `mcp.json` and dispatch `mcp__mock__echo`
    /// through the shared tool registry.
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

    /// U14 mcp-resilience: a config mixing a dead-at-start server (the
    /// 2026-07-16 incident shape), a hung-on-initialize server, and a healthy
    /// one must start DEGRADED — the healthy server's tools register, the
    /// broken ones are skipped with a per-server WARN, and nothing propagates
    /// upward as an error (let alone a process exit).
    #[test]
    fn discovery_degrades_per_server_without_blocking_healthy_tools() {
        let temp = tempfile::tempdir().expect("temp dir should be created");
        let config_path = temp.path().join("mcp.json");
        let script = repo_root()
            .join("tests")
            .join("fixtures")
            .join("mock_mcp.py");
        let config = json!({
            "mcpServers": {
                "dead": {
                    "command": "python3",
                    "args": [script.clone(), "exit-before-initialize"],
                    "enabled": true,
                    "timeout_seconds": 5
                },
                "hung": {
                    "command": "python3",
                    "args": [script.clone(), "silent"],
                    "enabled": true,
                    "timeout_seconds": 1
                },
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
            .expect("degraded discovery must not surface as an error");

        assert_eq!(registered, 1, "only the healthy server's tool registers");
        let names = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["mcp__mock__echo".to_string()]);
    }

    /// Public tool names reject server/tool parts with spaces or other unsafe
    /// characters before registration.
    #[test]
    fn rejects_unsafe_public_tool_name_parts() {
        let error = public_tool_name("bad server", "echo")
            .expect_err("server names with spaces should be rejected");
        assert!(
            error.to_string().contains("unsupported characters"),
            "unexpected error: {error}"
        );
    }

    /// Resolve the crate root via `CARGO_MANIFEST_DIR` for fixture paths in tests.
    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    // --- Agentic readiness probe (C4 semantics) ------------------------------
    //
    // `ready` is decided by the CORE capability gate (provider + key + native
    // tools), supplied here as an explicit `CoreReadiness` so the tests stay
    // deterministic and free of process-env / Keychain coupling. The MCP rows are
    // informational and must never flip the verdict. The global runtime cache is
    // never populated for these server names by any other test, so operator rows
    // read as "configured, agent not started" or "not configured (optional)".

    use super::CoreReadiness;

    /// Locate a labelled readiness/status row or panic with the missing label.
    fn find_row<'a>(
        report: &'a super::AgenticReadinessReport,
        label: &str,
    ) -> &'a super::McpStatusRow {
        report
            .summary_rows()
            .iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| panic!("row '{label}' present"))
    }

    /// Core gate that PASSES: provider access available, native tools present.
    fn core_ready() -> CoreReadiness {
        CoreReadiness {
            provider_label: "OpenAI (Responses)".to_string(),
            key_env_key: "LLM_OPENAI_API_KEY".to_string(),
            provider_access_available: true,
            native_tool_count: 10,
            configured_workspace_roots: vec!["~/Git".to_string()],
            tool_workspace_roots: vec!["~/Git".to_string()],
        }
    }

    /// Live core probe must report a non-zero native tool count and non-empty
    /// provider/key labels in a healthy build.
    #[test]
    fn probe_core_readiness_counts_native_tools() {
        // The real native tool set is compiled in; the count must be non-zero so
        // the core gate never fails purely on "no tools" in a healthy build.
        let snapshot = codescribe_core::config::Config::load_runtime_snapshot()
            .expect("canonical runtime settings should load");
        let core = super::probe_core_readiness(&snapshot);
        assert!(
            core.native_tool_count > 0,
            "native tools should be registered"
        );
        assert!(!core.provider_label.is_empty());
        assert!(!core.key_env_key.is_empty());
    }

    #[test]
    fn core_readiness_requires_a_model_for_a_key_optional_custom_provider() {
        use codescribe_core::config::{CapturedRuntimeInputs, Config};
        use codescribe_core::llm::provider::{CustomProvider, WireFamily};

        let root = tempfile::tempdir().expect("isolated runtime root");
        let mut input = CapturedRuntimeInputs::defaults_at(root.path().to_path_buf(), 1);
        let provider = CustomProvider::new(
            "Readiness fixture",
            WireFamily::OpenAiResponses,
            "http://localhost:8080/v1",
        )
        .expect("valid custom provider");
        input.user_settings.llm_assistive_provider = Some(format!("custom:{}", provider.id));
        input.user_settings.llm_custom_providers = vec![provider];

        let without_model = Config::runtime_snapshot_from_captured(input.clone());
        let lane = without_model.llm_lanes().assistive();
        assert!(lane.request_available(), "the custom endpoint needs no key");
        assert!(!lane.available(), "the loader refuses a missing model");
        assert!(
            !super::probe_core_readiness(&without_model).provider_access_available,
            "credential access alone cannot make an unusable lane ready"
        );

        input.user_settings.llm_assistive_model = Some("fixture-model".to_string());
        let with_model = Config::runtime_snapshot_from_captured(input);
        assert!(with_model.llm_lanes().assistive().available());
        assert!(super::probe_core_readiness(&with_model).provider_access_available);
    }

    /// A passing core gate is READY with zero operator MCP tooling; MCP absence
    /// cannot flip the agentic verdict.
    #[test]
    fn readiness_is_driven_by_core_gate_not_operator_tooling() {
        // Empty MCP config (no operator tooling at all) but a passing core gate:
        // the agent is READY. Operator tooling is optional context.
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // never created

        let report = probe_agentic_readiness_at(&path, core_ready());
        assert!(
            report.is_ready(),
            "core gate passing must be READY even with zero operator tooling: {:?}",
            report
                .summary_rows()
                .iter()
                .map(|r| format!("{}={}", r.label, r.value))
                .collect::<Vec<_>>()
        );
        let verdict = find_row(&report, "Agentic readiness:");
        assert_eq!(verdict.tone, McpRowTone::Good);
        assert!(verdict.value.contains("ready"), "got: {}", verdict.value);
        let provider = find_row(&report, "Provider:");
        assert_eq!(provider.tone, McpRowTone::Good);
        assert!(
            provider.value.contains("access available") && !provider.value.contains("key set"),
            "got: {}",
            provider.value
        );
    }

    /// Every readiness row carries a stable facet + state and the structured
    /// parts the Settings layer renders from; the English `value` is never
    /// the only carrier of a count or a provider name.
    #[test]
    fn readiness_rows_carry_facet_state_and_structured_parts() {
        use super::{McpStatusFacet, McpStatusState};
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // never created

        let report = probe_agentic_readiness_at(&path, core_ready());
        let verdict = find_row(&report, "Agentic readiness:");
        assert_eq!(verdict.facet, McpStatusFacet::Readiness);
        assert_eq!(verdict.state, McpStatusState::Ready);
        assert_eq!(verdict.subject, core_ready().provider_label);
        assert_eq!(verdict.count, Some(core_ready().native_tool_count as u32));

        let provider = find_row(&report, "Provider:");
        assert_eq!(provider.facet, McpStatusFacet::Provider);
        assert_eq!(provider.state, McpStatusState::AccessAvailable);
        assert_eq!(provider.detail, core_ready().key_env_key);

        let tools = find_row(&report, "Native tools:");
        assert_eq!(tools.facet, McpStatusFacet::NativeTools);
        assert_eq!(tools.state, McpStatusState::Available);

        let roots = find_row(&report, "Workspace roots:");
        assert_eq!(roots.facet, McpStatusFacet::WorkspaceRoots);
        assert_eq!(roots.state, McpStatusState::Synchronized);
        assert_eq!(
            roots.count,
            Some(core_ready().configured_workspace_roots.len() as u32)
        );

        for (label, facet) in [
            ("Vibecrafted runtime:", McpStatusFacet::VibecraftedRuntime),
            ("AICX MCP:", McpStatusFacet::AicxMcp),
            ("Loctree MCP:", McpStatusFacet::LoctreeMcp),
            ("PRView integration:", McpStatusFacet::PrviewIntegration),
        ] {
            let row = find_row(&report, label);
            assert_eq!(row.facet, facet, "{label}");
            assert_eq!(row.state, McpStatusState::NotConfigured, "{label}");
        }
    }

    /// Vibecrafted/AICX/Loctree/PRView rows stay Neutral/optional when unconfigured
    /// and never block readiness.
    #[test]
    fn operator_tooling_absent_is_neutral_optional_and_non_blocking() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // never created

        let report = probe_agentic_readiness_at(&path, core_ready());
        assert!(report.is_ready());
        for label in [
            "Vibecrafted runtime:",
            "AICX MCP:",
            "Loctree MCP:",
            "PRView integration:",
        ] {
            let row = find_row(&report, label);
            assert_eq!(
                row.tone,
                McpRowTone::Neutral,
                "{label} must be neutral/optional"
            );
            assert!(row.value.contains("optional"), "{label} got: {}", row.value);
        }
    }

    /// Full MCP substrate cannot rescue readiness when provider access is unavailable.
    #[test]
    fn unavailable_provider_access_blocks_readiness_even_with_full_substrate() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        // Full operator substrate present — but the core gate has no provider access.
        let config = json!({
            "mcpServers": {
                "vibecrafted-mcp": { "command": "vibecrafted-mcp", "enabled": true },
                "aicx-mcp": { "command": "aicx-mcp", "enabled": true },
                "loctree-mcp": { "command": "loctree-mcp", "enabled": true },
                "prview": { "command": "prview", "args": ["mcp"], "enabled": true }
            }
        });
        fs::write(&path, config.to_string()).expect("write config");

        let core = CoreReadiness {
            provider_access_available: false,
            ..core_ready()
        };
        let report = probe_agentic_readiness_at(&path, core);
        assert!(
            !report.is_ready(),
            "unavailable provider access must block readiness regardless of MCP substrate"
        );
        let verdict = find_row(&report, "Agentic readiness:");
        assert_eq!(verdict.tone, McpRowTone::Bad);
        assert!(
            verdict.value.contains("provider access unavailable"),
            "got: {}",
            verdict.value
        );
        let provider = find_row(&report, "Provider:");
        assert_eq!(provider.tone, McpRowTone::Bad);
        assert!(
            provider.value.contains("LLM_OPENAI_API_KEY"),
            "provider row should name the missing key account, got: {}",
            provider.value
        );
    }

    /// Zero native tools fail the core gate and block agentic readiness.
    #[test]
    fn no_native_tools_blocks_readiness() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        let core = CoreReadiness {
            native_tool_count: 0,
            ..core_ready()
        };
        let report = probe_agentic_readiness_at(&path, core);
        assert!(!report.is_ready(), "zero native tools must block readiness");
        let tools = find_row(&report, "Native tools:");
        assert_eq!(tools.tone, McpRowTone::Bad);
        let verdict = find_row(&report, "Agentic readiness:");
        assert!(
            verdict.value.contains("no native tools"),
            "got: {}",
            verdict.value
        );
    }

    /// Tool workspace roots that resolve fewer paths than Settings must block
    /// readiness with an explicit mismatch row.
    #[test]
    fn workspace_root_mismatch_blocks_readiness() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        let core = CoreReadiness {
            configured_workspace_roots: vec![
                "~/Git".to_string(),
                "/Users/op/workspace/projects".to_string(),
            ],
            tool_workspace_roots: vec!["~/Git".to_string()],
            ..core_ready()
        };

        let report = probe_agentic_readiness_at(&path, core);
        assert!(
            !report.is_ready(),
            "readiness must not claim READY when native tools resolve fewer roots than Settings"
        );
        let roots = find_row(&report, "Workspace roots:");
        assert_eq!(roots.tone, McpRowTone::Bad);
        assert!(roots.value.contains("mismatch"), "got: {}", roots.value);
        let verdict = find_row(&report, "Agentic readiness:");
        assert!(
            verdict.value.contains("workspace roots differ"),
            "got: {}",
            verdict.value
        );
    }

    /// Configured PRView is informational (Warn before discovery) and never gates
    /// the ready verdict.
    #[test]
    fn configured_prview_server_is_informational_good_or_warn() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        // Canonical PRView wiring: {"command":"prview","args":["mcp"]}.
        let config = json!({
            "mcpServers": {
                "prview": { "command": "prview", "args": ["mcp"], "enabled": true }
            }
        });
        fs::write(&path, config.to_string()).expect("write config");

        let report = probe_agentic_readiness_at(&path, core_ready());
        assert!(report.is_ready(), "PRView presence never gates readiness");
        let prview = find_row(&report, "PRView integration:");
        // Configured but no runtime discovery yet → warn, and it names the server.
        assert_eq!(prview.tone, McpRowTone::Warn);
        assert!(
            prview.value.contains("configured") && prview.value.contains("prview"),
            "got: {}",
            prview.value
        );
    }

    /// Broken `mcp.json` warns on the config row but leaves readiness decided by
    /// the core gate alone.
    #[test]
    fn broken_mcp_json_is_informational_not_blocking() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        fs::write(&path, "{ not valid json").expect("write garbage");

        // A broken config no longer blocks readiness — MCP is optional now.
        let report = probe_agentic_readiness_at(&path, core_ready());
        assert!(
            report.is_ready(),
            "a broken mcp.json must not sink a working agent"
        );
        let note = find_row(&report, "MCP config:");
        assert_eq!(note.tone, McpRowTone::Warn);
        assert!(note.value.contains("parse error"), "got: {}", note.value);
    }

    /// Basic-lane MCP probe keeps a single Neutral/optional row when `mcp.json`
    /// is missing.
    #[test]
    fn basic_probe_stays_neutral_for_missing_config() {
        // The Basic-lane probe is unchanged: a missing mcp.json is a single
        // neutral/optional row, never an error.
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // never created

        let basic = probe_mcp_status_at(&path);
        assert_eq!(basic.summary_rows().len(), 1);
        assert_eq!(basic.summary_rows()[0].tone, McpRowTone::Neutral);
    }
}

#[cfg(test)]
#[path = "mcp_identity_tests.rs"]
mod identity_tests;
