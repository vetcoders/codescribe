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

/// One Agentic prerequisite the readiness probe classifies, paired with the MCP
/// server name that satisfies it.
///
/// Repo truth (loctree literal scan, 2026-06-26): these three server names are
/// the only first-party agentic surfaces referenced anywhere in CodeScribe
/// (`app/controller/helpers.rs`, `app/ui/voice_chat/tool_activity.rs`). PRView
/// is deliberately absent here — it has no known MCP server name or local
/// command in repo truth and is handled separately by `classify_prview`.
const AGENTIC_PREREQS: &[(&str, &str)] = &[
    ("Vibecrafted runtime:", "vibecrafted-mcp"),
    ("AICX MCP:", "aicx-mcp"),
    ("Loctree MCP:", "loctree-mcp"),
];

/// Mode-aware readiness verdict for the Agentic operating lane.
///
/// Distinct from [`McpStatusReport`] (the Basic-mode config probe) so the two
/// lanes can never bleed into each other: Basic stays neutral/optional, Agentic
/// gets a hard ready/not-ready verdict. Reuses [`McpStatusRow`]/[`McpRowTone`]
/// so there is no parallel tone system.
pub struct AgenticReadinessReport {
    pub config_path_display: String,
    ready: bool,
    rows: Vec<McpStatusRow>,
}

impl AgenticReadinessReport {
    pub fn summary_rows(&self) -> &[McpStatusRow] {
        &self.rows
    }

    /// `true` only when every gating prerequisite is non-blocking. The agentic
    /// substrate is Vibecrafted + AICX + Loctree + PRView; all four are required.
    /// A missing/disabled/failed PRView integration flips this to `false` just
    /// like any other prerequisite — PRView is minimum substrate, not decoration.
    pub fn is_ready(&self) -> bool {
        self.ready
    }
}

/// Classify one gating prerequisite against config + cached runtime discovery.
///
/// Returns the display row and whether the prerequisite is *blocking* (i.e. it
/// flips overall readiness to not-ready). The value strings carry a stable
/// keyword (`not configured` / `disabled` / `failed` / `ready` / `configured`)
/// plus an actionable hint, mirroring the honesty of [`probe_mcp_status_at`].
fn classify_prereq(
    label: &str,
    server_name: &str,
    config: &McpConfigFile,
    runtime: &BTreeMap<String, ServerRuntime>,
) -> (McpStatusRow, bool) {
    let configured = config.servers.get(server_name);
    let (value, tone, blocking) = match (configured, runtime.get(server_name)) {
        // Not present in mcp.json at all — the substrate for this prereq is absent.
        (None, _) => (
            format!("not configured — add \"{server_name}\" to ~/.codescribe/mcp.json"),
            McpRowTone::Bad,
            true,
        ),
        // Real discovery succeeded — tools are live.
        (Some(_), Some(ServerRuntime::Tools(count))) => (
            format!("ready — {count} tool(s) live"),
            McpRowTone::Good,
            false,
        ),
        // Configured but discovery failed: surface the concrete reason.
        (Some(_), Some(ServerRuntime::Failed(reason))) => {
            (format!("failed: {reason}"), McpRowTone::Bad, true)
        }
        // Present in config but disabled (either via cache or the `enabled` flag):
        // a disabled prerequisite blocks the agentic lane.
        (Some(cfg), runtime_state) => {
            let enabled = cfg.enabled.unwrap_or(true);
            if matches!(runtime_state, Some(ServerRuntime::Disabled)) || !enabled {
                (
                    format!("disabled — set \"enabled\": true for \"{server_name}\""),
                    McpRowTone::Bad,
                    true,
                )
            } else {
                // Config is correct; the agent runtime simply has not run discovery
                // yet. Not blocking — the substrate is present.
                (
                    "configured — agent not started yet".to_string(),
                    McpRowTone::Warn,
                    false,
                )
            }
        }
    };
    (
        McpStatusRow {
            label: label.to_string(),
            value,
            tone,
        },
        blocking,
    )
}

/// Classify PRView readiness as a **required** agentic prerequisite.
///
/// Repo truth (loctree literal scan, 2026-06-26): there is NO `prview` MCP
/// server name and no local prview command anywhere in CodeScribe. We refuse to
/// invent a binary name. If a user has manually wired a server whose name
/// contains "prview" we honour that real config; otherwise we report the exact
/// evidence — missing integration — and never fake green readiness.
///
/// Returns the display row and whether the prerequisite is *blocking*, matching
/// [`classify_prereq`]'s contract: a missing/disabled/failed PRView integration
/// blocks Agentic readiness because PRView is minimum substrate, not decoration.
fn classify_prview(
    config: &McpConfigFile,
    runtime: &BTreeMap<String, ServerRuntime>,
) -> (McpStatusRow, bool) {
    let detected = config
        .servers
        .iter()
        .find(|(name, _)| name.to_ascii_lowercase().contains("prview"));
    let (value, tone, blocking) = match detected {
        Some((name, cfg)) => match runtime.get(name) {
            // Real discovery succeeded — PRView tools are live.
            Some(ServerRuntime::Tools(count)) => (
                format!("ready — {count} tool(s) live (via \"{name}\")"),
                McpRowTone::Good,
                false,
            ),
            // Configured but discovery failed: surface the concrete reason.
            Some(ServerRuntime::Failed(reason)) => {
                (format!("failed: {reason}"), McpRowTone::Bad, true)
            }
            // Disabled (via cache or the `enabled` flag) blocks the agentic lane.
            Some(ServerRuntime::Disabled) => (
                format!("disabled — set \"enabled\": true for \"{name}\""),
                McpRowTone::Bad,
                true,
            ),
            None => {
                if cfg.enabled.unwrap_or(true) {
                    // Config is correct; the agent runtime has not run discovery
                    // yet. Not blocking — the substrate is present.
                    (
                        format!("configured — agent not started yet (via \"{name}\")"),
                        McpRowTone::Warn,
                        false,
                    )
                } else {
                    (
                        format!("disabled — set \"enabled\": true for \"{name}\""),
                        McpRowTone::Bad,
                        true,
                    )
                }
            }
        },
        // `missing_prview_integration`: no MCP server, no local command. Honest
        // evidence preserved (loctree literal `prview` = 0 production occurrences),
        // but PRView is required substrate, so its absence blocks readiness.
        None => (
            "missing (required) — no PRView MCP server or local command found".to_string(),
            McpRowTone::Bad,
            true,
        ),
    };
    (
        McpStatusRow {
            label: "PRView integration:".to_string(),
            value,
            tone,
        },
        blocking,
    )
}

/// Agentic-lane readiness probe: classifies the agentic substrate prerequisites
/// (Vibecrafted runtime, AICX MCP, Loctree MCP, PRView integration).
///
/// Only meaningful when the user chose the Agentic operating lane. Basic mode
/// must keep calling [`probe_mcp_status`] so a missing `mcp.json` stays a
/// neutral/optional state rather than a hard not-ready verdict.
pub fn probe_agentic_readiness() -> AgenticReadinessReport {
    let path = match codescribe_core::mcp::default_mcp_config_path() {
        Ok(path) => path,
        Err(error) => {
            return AgenticReadinessReport {
                config_path_display: "unavailable".to_string(),
                ready: false,
                rows: vec![McpStatusRow {
                    label: "Agentic readiness:".to_string(),
                    value: format!("not ready — config path unavailable: {error}"),
                    tone: McpRowTone::Bad,
                }],
            };
        }
    };
    probe_agentic_readiness_at(&path)
}

fn probe_agentic_readiness_at(path: &Path) -> AgenticReadinessReport {
    let config_path_display = path.display().to_string();

    // Missing config in the Agentic lane is NOT neutral (that is Basic's
    // contract): the substrate is simply absent, so classify against an empty
    // config and every prereq reports "not configured" → not ready.
    let config = if !path.exists() {
        McpConfigFile {
            servers: Default::default(),
        }
    } else {
        match McpConfigFile::load(path) {
            Ok(config) => config,
            Err(error) => {
                return AgenticReadinessReport {
                    config_path_display,
                    ready: false,
                    rows: vec![
                        McpStatusRow {
                            label: "Agentic readiness:".to_string(),
                            value: "not ready — config error".to_string(),
                            tone: McpRowTone::Bad,
                        },
                        McpStatusRow {
                            label: "Config error:".to_string(),
                            value: anyhow_root_cause(&error),
                            tone: McpRowTone::Bad,
                        },
                    ],
                };
            }
        }
    };

    let runtime = runtime_cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();

    let mut prereq_rows = Vec::with_capacity(AGENTIC_PREREQS.len() + 1);
    let mut blocking = 0usize;
    for (label, server_name) in AGENTIC_PREREQS {
        let (row, is_blocking) = classify_prereq(label, server_name, &config, &runtime);
        if is_blocking {
            blocking += 1;
        }
        prereq_rows.push(row);
    }
    let (prview_row, prview_blocking) = classify_prview(&config, &runtime);
    if prview_blocking {
        blocking += 1;
    }
    prereq_rows.push(prview_row);

    let ready = blocking == 0;
    let verdict = if ready {
        McpStatusRow {
            label: "Agentic readiness:".to_string(),
            value: "ready — full agentic substrate present".to_string(),
            tone: McpRowTone::Good,
        }
    } else {
        McpStatusRow {
            label: "Agentic readiness:".to_string(),
            value: format!("not ready — {blocking} prerequisite(s) missing"),
            tone: McpRowTone::Bad,
        }
    };

    let mut rows = Vec::with_capacity(prereq_rows.len() + 1);
    rows.push(verdict);
    rows.extend(prereq_rows);

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

    use super::{
        McpRowTone, probe_agentic_readiness_at, probe_mcp_status_at, public_tool_name,
        register_mcp_tools_from_config_path,
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

    // --- Agentic readiness probe (W1-C2) -------------------------------------
    //
    // These exercise `probe_agentic_readiness_at` with real prereq server names
    // (`vibecrafted-mcp` / `aicx-mcp` / `loctree-mcp`). The global runtime cache
    // is never populated for those names by any other test, so the readiness
    // probe sees them as "configured, agent not started" — the deterministic,
    // ordering-safe baseline.

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

    #[test]
    fn basic_probe_stays_neutral_while_agentic_is_not_ready_for_missing_config() {
        // Same nonexistent path, two lanes: Basic must remain optional/neutral,
        // Agentic must report a hard not-ready verdict.
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // never created

        let basic = probe_mcp_status_at(&path);
        assert_eq!(basic.summary_rows().len(), 1);
        assert_eq!(basic.summary_rows()[0].tone, McpRowTone::Neutral);

        let agentic = probe_agentic_readiness_at(&path);
        assert!(
            !agentic.is_ready(),
            "agentic must not be ready without config"
        );
        let verdict = find_row(&agentic, "Agentic readiness:");
        assert_eq!(verdict.tone, McpRowTone::Bad);
        assert!(
            verdict.value.contains("not ready"),
            "got: {}",
            verdict.value
        );
    }

    #[test]
    fn agentic_missing_config_classifies_every_prereq_as_not_configured() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // never created

        let report = probe_agentic_readiness_at(&path);
        assert!(!report.is_ready());
        for label in ["Vibecrafted runtime:", "AICX MCP:", "Loctree MCP:"] {
            let row = find_row(&report, label);
            assert_eq!(row.tone, McpRowTone::Bad, "{label} should block");
            assert!(
                row.value.contains("not configured"),
                "{label} got: {}",
                row.value
            );
        }
    }

    #[test]
    fn agentic_recognizes_configured_full_substrate() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        // Full required substrate: Vibecrafted + AICX + Loctree + a PRView-like
        // server. A user who wires a "prview" server satisfies the prerequisite.
        let config = json!({
            "mcpServers": {
                "vibecrafted-mcp": { "command": "vibecrafted-mcp", "enabled": true },
                "aicx-mcp": { "command": "aicx-mcp", "enabled": true },
                "loctree-mcp": { "command": "loctree-mcp", "enabled": true },
                "vista-prview": { "command": "vista-prview", "enabled": true }
            }
        });
        fs::write(&path, config.to_string()).expect("write config");

        let report = probe_agentic_readiness_at(&path);
        // All four prereqs are configured + enabled (no runtime discovery yet),
        // so none block: the full agentic substrate is recognized as present.
        assert!(
            report.is_ready(),
            "configured full substrate should be ready: {:?}",
            report
                .summary_rows()
                .iter()
                .map(|r| format!("{}={}", r.label, r.value))
                .collect::<Vec<_>>()
        );
        for label in ["AICX MCP:", "Loctree MCP:"] {
            let row = find_row(&report, label);
            assert_eq!(row.tone, McpRowTone::Warn);
            assert!(
                row.value.contains("configured"),
                "{label} got: {}",
                row.value
            );
        }
        // A configured/enabled PRView-like server satisfies the PRView prereq.
        let prview = find_row(&report, "PRView integration:");
        assert_eq!(prview.tone, McpRowTone::Warn);
        assert!(
            prview.value.contains("configured") && prview.value.contains("vista-prview"),
            "PRView-like server should satisfy the prerequisite, got: {}",
            prview.value
        );
    }

    #[test]
    fn agentic_disabled_prview_server_blocks_readiness() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        // Core substrate present, but the wired PRView-like server is disabled —
        // a disabled required prerequisite must block readiness.
        let config = json!({
            "mcpServers": {
                "vibecrafted-mcp": { "command": "vibecrafted-mcp", "enabled": true },
                "aicx-mcp": { "command": "aicx-mcp", "enabled": true },
                "loctree-mcp": { "command": "loctree-mcp", "enabled": true },
                "vista-prview": { "command": "vista-prview", "enabled": false }
            }
        });
        fs::write(&path, config.to_string()).expect("write config");

        let report = probe_agentic_readiness_at(&path);
        assert!(
            !report.is_ready(),
            "a disabled PRView server must block readiness"
        );
        let prview = find_row(&report, "PRView integration:");
        assert_eq!(prview.tone, McpRowTone::Bad);
        assert!(prview.value.contains("disabled"), "got: {}", prview.value);
    }

    #[test]
    fn agentic_disabled_prereq_blocks_readiness() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        let config = json!({
            "mcpServers": {
                "vibecrafted-mcp": { "command": "vibecrafted-mcp", "enabled": true },
                "aicx-mcp": { "command": "aicx-mcp", "enabled": false },
                "loctree-mcp": { "command": "loctree-mcp", "enabled": true }
            }
        });
        fs::write(&path, config.to_string()).expect("write config");

        let report = probe_agentic_readiness_at(&path);
        assert!(!report.is_ready(), "a disabled prereq must block readiness");
        let row = find_row(&report, "AICX MCP:");
        assert_eq!(row.tone, McpRowTone::Bad);
        assert!(row.value.contains("disabled"), "got: {}", row.value);
    }

    #[test]
    fn agentic_prview_missing_blocks_readiness() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        // Core substrate present, but no PRView surface anywhere — this is the
        // repo-truth baseline (loctree literal `prview` = 0 production hits).
        let config = json!({
            "mcpServers": {
                "vibecrafted-mcp": { "command": "vibecrafted-mcp", "enabled": true },
                "aicx-mcp": { "command": "aicx-mcp", "enabled": true },
                "loctree-mcp": { "command": "loctree-mcp", "enabled": true }
            }
        });
        fs::write(&path, config.to_string()).expect("write config");

        let report = probe_agentic_readiness_at(&path);
        let row = find_row(&report, "PRView integration:");
        // Honest evidence preserved AND now flagged as a hard, blocking gap.
        assert_eq!(row.tone, McpRowTone::Bad);
        assert!(
            row.value.contains("missing") && row.value.contains("PRView"),
            "PRView must be explicitly classified as missing, got: {}",
            row.value
        );
        // PRView is required substrate: its absence blocks Agentic readiness even
        // when Vibecrafted / AICX / Loctree are all present.
        assert!(
            !report.is_ready(),
            "missing PRView must block agentic readiness"
        );
    }

    #[test]
    fn agentic_parse_error_is_not_ready_with_concrete_reason() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        fs::write(&path, "{ not valid json").expect("write garbage");

        let report = probe_agentic_readiness_at(&path);
        assert!(!report.is_ready());
        let row = find_row(&report, "Config error:");
        assert_eq!(row.tone, McpRowTone::Bad);
        assert!(!row.value.is_empty());
    }
}
