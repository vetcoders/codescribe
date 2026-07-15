use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};
use std::thread;

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use codescribe_core::llm::lane_truth;
#[cfg(test)]
use codescribe_core::llm::provider::LlmMode;
use codescribe_core::llm::provider::ProviderKind;
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
    pub fn summary_rows(&self) -> &[McpStatusRow] {
        &self.rows
    }

    /// `true` when an `mcp.json` exists with at least one server defined (or when
    /// a present config failed to load — the file is still there). `false` only
    /// when there is no config yet: missing file or present-but-no-servers.
    pub fn configured(&self) -> bool {
        self.configured
    }

    fn single(
        config_path_display: String,
        configured: bool,
        label: &str,
        value: String,
        tone: McpRowTone,
    ) -> Self {
        Self {
            config_path_display,
            configured,
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
                true,
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
            false,
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
                true,
                "Config error:",
                anyhow_root_cause(&error),
                McpRowTone::Bad,
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
        configured: true,
        rows,
    }
}

/// Operator-tooling MCP servers surfaced as INFORMATIONAL rows in the readiness
/// panel, each paired with the server name that satisfies it.
///
/// These are Vetcoders operator surfaces (Vibecrafted / AICX / Loctree); an
/// end-user install will not have them. Per the C4 readiness-semantics decision
/// they are context only and NEVER gate `ready` — the core capability gate
/// (provider + key + native tools) is the sole arbiter of readiness. PRView is
/// handled separately by [`classify_prview`], also as optional context.
const AGENTIC_PREREQS: &[(&str, &str)] = &[
    ("Vibecrafted runtime:", "vibecrafted-mcp"),
    ("AICX MCP:", "aicx-mcp"),
    ("Loctree MCP:", "loctree-mcp"),
];

/// Core capability gate — the REAL ability of the agent to act. This is the only
/// input that decides `ready`: a configured assistive-lane provider whose API
/// key is present, plus the compiled-in native tool set. Operator tooling (MCP
/// servers) is informational and never enters this verdict.
#[derive(Debug, Clone)]
pub struct CoreReadiness {
    /// Display name of the resolved assistive-lane provider.
    pub provider_label: String,
    /// Keychain/env account holding that provider's assistive key.
    pub key_env_key: String,
    /// Whether that key is present (non-empty) in env or Keychain.
    pub key_set: bool,
    /// Number of native (compiled-in) tools available to the agent.
    pub native_tool_count: usize,
}

/// Probe the core capability gate from live process state: the configured
/// assistive provider ([`lane_truth::provider`]), whether its key is set, and
/// the count of native tools. Cheap — local config/secret reads plus building an
/// in-memory registry (no server spawning, no network).
pub fn probe_core_readiness() -> CoreReadiness {
    let snapshot = lane_truth::lane_truth_snapshot(
        lane_truth::LaneTruthLane::Assistive,
        &codescribe_core::config::Config::load(),
    );
    let provider = ProviderKind::from_str(&snapshot.provider_id)
        .expect("lane truth must emit a canonical provider id");
    assemble_core_readiness(provider, snapshot.key_account, snapshot.key_present)
}

#[cfg(test)]
fn probe_core_readiness_with_secret(
    resolve_secret: impl FnOnce(&str) -> Option<String>,
) -> CoreReadiness {
    let provider = lane_truth::provider(LlmMode::Assistive);
    let key_env_key = provider.api_key_env_key().to_string();
    let key_set = resolve_secret(&key_env_key).is_some();

    assemble_core_readiness(provider, key_env_key, key_set)
}

fn assemble_core_readiness(
    provider: ProviderKind,
    key_env_key: String,
    key_set: bool,
) -> CoreReadiness {
    let mut registry = ToolRegistry::new();
    super::register_native_tools(&mut registry);
    let native_tool_count = registry.definitions().len();

    CoreReadiness {
        provider_label: provider.display_name().to_string(),
        key_env_key,
        key_set,
        native_tool_count,
    }
}

/// Readiness verdict for the Agentic operating lane.
///
/// `ready` is decided SOLELY by the core capability gate ([`CoreReadiness`]:
/// assistive provider configured + its API key set + native tools compiled in).
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
    pub fn summary_rows(&self) -> &[McpStatusRow] {
        &self.rows
    }

    /// `true` only when the core capability gate passes: a configured assistive
    /// provider with its API key set and at least one native tool available.
    /// Operator-tooling MCP rows are informational and never affect this verdict.
    pub fn is_ready(&self) -> bool {
        self.ready
    }
}

/// Classify one operator-tooling MCP server as an INFORMATIONAL row. Never
/// blocking: a missing/failed/disabled operator server does not affect agent
/// readiness (the core gate owns that). Absent → neutral "not configured
/// (optional)"; live → good; configured-but-unstarted or failed → warn.
fn classify_operator_tool(
    label: &str,
    server_name: &str,
    config: &McpConfigFile,
    runtime: &BTreeMap<String, ServerRuntime>,
) -> McpStatusRow {
    let configured = config.servers.get(server_name);
    let (value, tone) = match (configured, runtime.get(server_name)) {
        // Not present in mcp.json — optional operator tooling is simply absent.
        (None, _) => ("not configured (optional)".to_string(), McpRowTone::Neutral),
        // Real discovery succeeded — tools are live.
        (Some(_), Some(ServerRuntime::Tools(count))) => {
            (format!("ready — {count} tool(s) live"), McpRowTone::Good)
        }
        // Configured but discovery failed: surface the concrete reason (warn, not
        // blocking — the agent still works without this operator surface).
        (Some(_), Some(ServerRuntime::Failed(reason))) => {
            (format!("failed: {reason}"), McpRowTone::Warn)
        }
        (Some(cfg), runtime_state) => {
            let enabled = cfg.enabled.unwrap_or(true);
            if matches!(runtime_state, Some(ServerRuntime::Disabled)) || !enabled {
                ("disabled".to_string(), McpRowTone::Neutral)
            } else {
                (
                    "configured — agent not started yet".to_string(),
                    McpRowTone::Warn,
                )
            }
        }
    };
    McpStatusRow {
        label: label.to_string(),
        value,
        tone,
    }
}

/// Classify PRView as an INFORMATIONAL row. Per the C4 decision PRView is
/// OPTIONAL, not required substrate: its absence is a neutral "not configured
/// (optional)" and never blocks readiness. The canonical wiring is a `prview`
/// MCP server (`{"command":"prview","args":["mcp"]}`); we also honour any server
/// whose name contains "prview" so a manually wired entry is recognised.
fn classify_prview(
    config: &McpConfigFile,
    runtime: &BTreeMap<String, ServerRuntime>,
) -> McpStatusRow {
    let detected = config
        .servers
        .iter()
        .find(|(name, _)| name.to_ascii_lowercase().contains("prview"));
    let (value, tone) = match detected {
        Some((name, cfg)) => match runtime.get(name) {
            Some(ServerRuntime::Tools(count)) => (
                format!("ready — {count} tool(s) live (via \"{name}\")"),
                McpRowTone::Good,
            ),
            Some(ServerRuntime::Failed(reason)) => (format!("failed: {reason}"), McpRowTone::Warn),
            Some(ServerRuntime::Disabled) => ("disabled".to_string(), McpRowTone::Neutral),
            None => {
                if cfg.enabled.unwrap_or(true) {
                    (
                        format!("configured — agent not started yet (via \"{name}\")"),
                        McpRowTone::Warn,
                    )
                } else {
                    ("disabled".to_string(), McpRowTone::Neutral)
                }
            }
        },
        None => ("not configured (optional)".to_string(), McpRowTone::Neutral),
    };
    McpStatusRow {
        label: "PRView integration:".to_string(),
        value,
        tone,
    }
}

/// Agentic-lane readiness probe: `ready` is the core capability gate (provider +
/// key + native tools); the MCP rows are informational context. See
/// [`AgenticReadinessReport`] for the semantics decision.
pub fn probe_agentic_readiness() -> AgenticReadinessReport {
    let core = probe_core_readiness();
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
            );
        }
    };
    probe_agentic_readiness_at(&path, core)
}

fn probe_agentic_readiness_at(path: &Path, core: CoreReadiness) -> AgenticReadinessReport {
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

    assemble_readiness(config_path_display, core, config, config_note)
}

/// Assemble the readiness report: the core-gate verdict + provider + native-tools
/// rows (which decide `ready`), followed by the informational operator-tooling
/// rows (which never do).
fn assemble_readiness(
    config_path_display: String,
    core: CoreReadiness,
    config: McpConfigFile,
    config_note: Option<String>,
) -> AgenticReadinessReport {
    let runtime = runtime_cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();

    let tools_present = core.native_tool_count > 0;
    let ready = core.key_set && tools_present;

    // ---- Core capability gate rows (these decide `ready`). ----
    let verdict = if ready {
        McpStatusRow {
            label: "Agentic readiness:".to_string(),
            value: format!(
                "ready — {} configured, key set, {} native tool(s)",
                core.provider_label, core.native_tool_count
            ),
            tone: McpRowTone::Good,
        }
    } else {
        let reason = if !core.key_set {
            format!("assistive API key missing (set {})", core.key_env_key)
        } else {
            "no native tools available".to_string()
        };
        McpStatusRow {
            label: "Agentic readiness:".to_string(),
            value: format!("not ready — {reason}"),
            tone: McpRowTone::Bad,
        }
    };

    let provider_row = McpStatusRow {
        label: "Provider:".to_string(),
        value: if core.key_set {
            format!("{} — key set", core.provider_label)
        } else {
            format!(
                "{} — key missing (set {})",
                core.provider_label, core.key_env_key
            )
        },
        tone: if core.key_set {
            McpRowTone::Good
        } else {
            McpRowTone::Bad
        },
    };

    let tools_row = McpStatusRow {
        label: "Native tools:".to_string(),
        value: format!("{} tool(s) available", core.native_tool_count),
        tone: if tools_present {
            McpRowTone::Good
        } else {
            McpRowTone::Bad
        },
    };

    let mut rows = Vec::with_capacity(AGENTIC_PREREQS.len() + 5);
    rows.push(verdict);
    rows.push(provider_row);
    rows.push(tools_row);

    // ---- Informational operator-tooling rows (never gate `ready`). ----
    if let Some(note) = config_note {
        rows.push(McpStatusRow {
            label: "MCP config:".to_string(),
            value: note,
            tone: McpRowTone::Warn,
        });
    }
    for (label, server_name) in AGENTIC_PREREQS {
        rows.push(classify_operator_tool(
            label,
            server_name,
            &config,
            &runtime,
        ));
    }
    rows.push(classify_prview(&config, &runtime));

    AgenticReadinessReport {
        config_path_display,
        ready,
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
    use serial_test::serial;

    use super::{
        McpRowTone, probe_agentic_readiness_at, probe_core_readiness_with_secret,
        probe_mcp_status_at, public_tool_name, register_mcp_tools_from_config_path,
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
        // No config yet → the onboarding step must offer the setup prompt.
        assert!(!report.configured());
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
        // File exists (user configured something) → show the error, not the
        // onboarding prompt.
        assert!(report.configured());
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
        // Present but empty config counts as "not configured yet" → prompt.
        assert!(!report.configured());
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
        // A config with at least one server → configured; no setup prompt.
        assert!(report.configured());
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

    // --- Agentic readiness probe (C4 semantics) ------------------------------
    //
    // `ready` is decided by the CORE capability gate (provider + key + native
    // tools), supplied here as an explicit `CoreReadiness` so the tests stay
    // deterministic and free of process-env / Keychain coupling. The MCP rows are
    // informational and must never flip the verdict. The global runtime cache is
    // never populated for these server names by any other test, so operator rows
    // read as "configured, agent not started" or "not configured (optional)".

    use super::CoreReadiness;

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

    /// Core gate that PASSES: provider configured, key set, native tools present.
    fn core_ready() -> CoreReadiness {
        CoreReadiness {
            provider_label: "OpenAI (Responses)".to_string(),
            key_env_key: "LLM_ASSISTIVE_API_KEY".to_string(),
            key_set: true,
            native_tool_count: 10,
        }
    }

    #[test]
    fn probe_core_readiness_counts_native_tools() {
        // The real native tool set is compiled in; the count must be non-zero so
        // the core gate never fails purely on "no tools" in a healthy build.
        let core = super::probe_core_readiness();
        assert!(
            core.native_tool_count > 0,
            "native tools should be registered"
        );
        assert!(!core.provider_label.is_empty());
        assert!(!core.key_env_key.is_empty());
    }

    #[test]
    #[serial]
    fn lane_truth_keychain_only_secret_sets_probe_core_readiness() {
        let _provider = EnvGuard::remove("LLM_ASSISTIVE_PROVIDER");
        let _key = EnvGuard::remove("LLM_ASSISTIVE_API_KEY");

        let core = probe_core_readiness_with_secret(|account| {
            (account == "LLM_ASSISTIVE_API_KEY").then(|| "keychain-only".to_string())
        });

        assert_eq!(core.key_env_key, "LLM_ASSISTIVE_API_KEY");
        assert!(
            core.key_set,
            "a Keychain-only secret must satisfy readiness"
        );
    }

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
            provider.value.contains("key set"),
            "got: {}",
            provider.value
        );
    }

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

    #[test]
    fn missing_assistive_key_blocks_readiness_even_with_full_substrate() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        // Full operator substrate present — but the core gate has no key.
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
            key_set: false,
            ..core_ready()
        };
        let report = probe_agentic_readiness_at(&path, core);
        assert!(
            !report.is_ready(),
            "no API key must block readiness regardless of MCP substrate"
        );
        let verdict = find_row(&report, "Agentic readiness:");
        assert_eq!(verdict.tone, McpRowTone::Bad);
        assert!(
            verdict.value.contains("key missing") || verdict.value.contains("key"),
            "got: {}",
            verdict.value
        );
        let provider = find_row(&report, "Provider:");
        assert_eq!(provider.tone, McpRowTone::Bad);
        assert!(
            provider.value.contains("LLM_ASSISTIVE_API_KEY"),
            "provider row should name the missing key account, got: {}",
            provider.value
        );
    }

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

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: this process-env test is serialized with `serial`.
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.as_deref() {
                Some(value) => {
                    // SAFETY: this process-env test is serialized with `serial`.
                    unsafe { std::env::set_var(self.key, value) };
                }
                None => {
                    // SAFETY: this process-env test is serialized with `serial`.
                    unsafe { std::env::remove_var(self.key) };
                }
            }
        }
    }
}
