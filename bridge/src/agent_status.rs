//! Agent-status surface — read-only UniFFI wrapper over the codescribe agentic
//! readiness + MCP status probes (`app/agent/tools/mcp.rs`). Sync-only: every
//! call is cheap disk I/O (reads/parses `mcp.json`, merges the last runtime
//! discovery snapshot; no server spawning). Split as its own bridge slice so the
//! Settings Engine panel can render honest agent-substrate state instead of the
//! probes staying built-but-dead.
//!
//! Nothing here mutates config — MCP editing is a separate cut. This slice only
//! reports what the core already knows.

use codescribe::agent::tools::mcp::{
    AgenticReadinessReport, McpRowTone, McpStatusReport, McpStatusRow, probe_agentic_readiness,
    probe_mcp_status,
};
use codescribe_core::agent::{ConnectorHealth, capability_matrix};
use codescribe_core::config::Config;
use codescribe_core::mcp::{default_mcp_config_path, list_servers};

/// Visual tone for one status row, mirrored 1:1 from the core [`McpRowTone`] so
/// the Settings layer maps it to concrete colors without depending on agent
/// tooling.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsMcpRowTone {
    Good,
    Warn,
    Bad,
    Neutral,
}

impl From<McpRowTone> for CsMcpRowTone {
    fn from(tone: McpRowTone) -> Self {
        match tone {
            McpRowTone::Good => CsMcpRowTone::Good,
            McpRowTone::Warn => CsMcpRowTone::Warn,
            McpRowTone::Bad => CsMcpRowTone::Bad,
            McpRowTone::Neutral => CsMcpRowTone::Neutral,
        }
    }
}

/// One labelled status line (label + value + tone) for the Settings UI.
#[derive(uniffi::Record)]
pub struct CsMcpStatusRow {
    pub label: String,
    pub value: String,
    pub tone: CsMcpRowTone,
}

impl From<&McpStatusRow> for CsMcpStatusRow {
    fn from(row: &McpStatusRow) -> Self {
        Self {
            label: row.label.clone(),
            value: row.value.clone(),
            tone: row.tone.into(),
        }
    }
}

/// Honest MCP config + runtime snapshot for the Settings "MCP servers" section.
/// A missing `mcp.json` degrades to a single neutral "MCP off (optional)" row —
/// never an error.
#[derive(uniffi::Record)]
pub struct CsMcpStatusReport {
    pub config_path_display: String,
    /// `false` when there is no MCP config yet (missing `mcp.json` or a present
    /// config with no servers). The onboarding readiness step uses this to choose
    /// between the status card and the "set up MCP servers" prompt.
    pub configured: bool,
    pub rows: Vec<CsMcpStatusRow>,
}

impl From<McpStatusReport> for CsMcpStatusReport {
    fn from(report: McpStatusReport) -> Self {
        let configured = report.configured();
        let rows = report
            .summary_rows()
            .iter()
            .map(CsMcpStatusRow::from)
            .collect();
        Self {
            config_path_display: report.config_path_display,
            configured,
            rows,
        }
    }
}

/// Agentic-lane readiness verdict + rows. `ready` reflects the CORE capability
/// gate only (assistive provider configured + its API key set + native tools
/// available); the MCP rows (Vibecrafted + AICX + Loctree + PRView) are
/// informational context and never flip `ready`. See the core
/// `AgenticReadinessReport` for the C4 semantics decision.
#[derive(uniffi::Record)]
pub struct CsAgenticReadiness {
    pub config_path_display: String,
    pub ready: bool,
    pub rows: Vec<CsMcpStatusRow>,
}

impl From<AgenticReadinessReport> for CsAgenticReadiness {
    fn from(report: AgenticReadinessReport) -> Self {
        let ready = report.is_ready();
        let rows = report
            .summary_rows()
            .iter()
            .map(CsMcpStatusRow::from)
            .collect();
        Self {
            config_path_display: report.config_path_display,
            ready,
            rows,
        }
    }
}

/// One row of the native / enhanced / unavailable capability matrix (V9).
#[derive(uniffi::Record)]
pub struct CsCapabilityRow {
    /// Canonical op id, e.g. `fs.list` / `repo.status`.
    pub op: String,
    /// `native` | `enhanced` | `unavailable`
    pub tier: String,
    /// Fulfilling provider label (not the model-facing contract).
    pub provider: String,
    pub native_tool: String,
    pub reason: String,
}

/// Read-only handle over the codescribe agent-status probes. Stateless: every
/// call re-reads config truth so Swift always sees on-disk state.
#[derive(uniffi::Object, Default)]
pub struct CodescribeAgentStatus {}

#[uniffi::export]
impl CodescribeAgentStatus {
    /// Construct the handle and ensure logging is initialised, since Swift may
    /// reach this before any other bridge entry point has run.
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self::default()
    }

    /// Basic-lane MCP status: reads/parses `mcp.json` + merges the last runtime
    /// discovery. Missing config → neutral optional row, never an error.
    pub fn mcp_status(&self) -> CsMcpStatusReport {
        probe_mcp_status().into()
    }

    /// Agentic-lane readiness. `ready` is the core capability gate (assistive
    /// provider + its API key + native tools); the MCP rows are informational.
    /// Loads `Config` first so Keychain-backed keys are populated into env and the
    /// core gate sees real key presence (mirrors `available_providers`).
    pub fn agentic_readiness(&self) -> CsAgenticReadiness {
        let _ = codescribe_core::config::Config::load();
        probe_agentic_readiness().into()
    }

    /// Provider-neutral capability matrix: native / enhanced / unavailable + reason.
    /// IntelliJ wrong-project or stale sessions are detected and bypassed.
    pub fn capability_matrix(&self) -> Vec<CsCapabilityRow> {
        let _ = codescribe_core::config::Config::load();
        let health = live_connector_health();
        capability_matrix(&health)
            .into_iter()
            .map(|row| CsCapabilityRow {
                op: row.op,
                tier: row.tier.as_str().to_string(),
                provider: row.provider.as_str().to_string(),
                native_tool: row.native_tool.unwrap_or_default(),
                reason: row.reason,
            })
            .collect()
    }
}

/// Derive connector health from the configured MCP servers for the capability
/// matrix.
///
/// Servers are matched by substring on their name, so operator-chosen names like
/// `loctree-mcp` or `my-intellij` still resolve. IntelliJ's project path needs a
/// second read of `mcp.json`: `list_servers` exposes env *keys* only, never their
/// values. Workspace roots are tilde-expanded here so the matrix shows real paths.
fn live_connector_health() -> ConnectorHealth {
    let servers = list_servers().unwrap_or_default();
    let mut intellij_healthy = false;
    let mut intellij_project_path = None;
    let mut loctree_healthy = false;
    let mut mcp_servers_with_tools = Vec::new();

    for server in &servers {
        if !server.enabled {
            continue;
        }
        let name_l = server.name.to_ascii_lowercase();
        if name_l.contains("loctree") {
            loctree_healthy = true;
            mcp_servers_with_tools.push(server.name.clone());
        }
        if name_l.contains("intellij") {
            intellij_healthy = true;
            mcp_servers_with_tools.push(server.name.clone());
            // Project path may live in env of the stdio server; list_servers
            // only exposes env *keys*, so read the config file for the value.
            if let Ok(path) = default_mcp_config_path()
                && let Ok(Some(cfg)) = codescribe_core::mcp::McpConfigFile::load_optional(&path)
                && let Some(server_cfg) = cfg.servers.get(&server.name)
            {
                intellij_project_path = server_cfg.env.get("IJ_MCP_SERVER_PROJECT_PATH").cloned();
            }
        }
    }

    let workspace_roots = Config::effective_agent_workspace_roots()
        .into_iter()
        .map(|root| {
            if let Some(rest) = root.strip_prefix("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    return format!("{home}/{rest}");
                }
            } else if root == "~"
                && let Ok(home) = std::env::var("HOME")
            {
                return home;
            }
            root
        })
        .collect();

    ConnectorHealth {
        loctree_healthy,
        intellij_healthy,
        intellij_project_path,
        workspace_roots,
        mcp_servers_with_tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe::agent::tools::mcp::{McpRowTone, McpStatusRow};

    #[test]
    fn tone_maps_one_to_one() {
        assert_eq!(CsMcpRowTone::from(McpRowTone::Good), CsMcpRowTone::Good);
        assert_eq!(CsMcpRowTone::from(McpRowTone::Warn), CsMcpRowTone::Warn);
        assert_eq!(CsMcpRowTone::from(McpRowTone::Bad), CsMcpRowTone::Bad);
        assert_eq!(
            CsMcpRowTone::from(McpRowTone::Neutral),
            CsMcpRowTone::Neutral
        );
    }

    #[test]
    fn row_conversion_preserves_fields() {
        let row = McpStatusRow {
            label: "loctree-mcp:".to_string(),
            value: "ready — 7 tool(s) live".to_string(),
            tone: McpRowTone::Good,
        };
        let cs = CsMcpStatusRow::from(&row);
        assert_eq!(cs.label, "loctree-mcp:");
        assert_eq!(cs.value, "ready — 7 tool(s) live");
        assert_eq!(cs.tone, CsMcpRowTone::Good);
    }

    // Degradation contract: the basic-lane probe always emits at least one row
    // (a present config lists servers; a missing one yields a single neutral
    // "MCP off" row). The FFI mapping must carry that row through with a
    // non-empty config-path label and never collapse to an empty report.
    #[test]
    fn mcp_status_report_maps_at_least_one_row() {
        let report = CodescribeAgentStatus::new().mcp_status();
        assert!(!report.rows.is_empty());
        assert!(!report.config_path_display.is_empty());
    }

    // The agentic readiness report always leads with a verdict row plus the
    // per-prerequisite rows, so the FFI view must carry several rows and a
    // boolean verdict without panicking on a bare environment.
    #[test]
    fn agentic_readiness_report_carries_verdict_and_rows() {
        let report = CodescribeAgentStatus::new().agentic_readiness();
        assert!(!report.rows.is_empty());
        // `ready` is a plain bool either way; this asserts the field is wired.
        let _ = report.ready;
    }
}
