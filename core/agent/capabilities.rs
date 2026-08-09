//! Provider-neutral capability broker for the Codescribe Agent.
//!
//! The model asks for stable canonical operations (`repo.status`, `fs.read`).
//! The broker records which provider fulfilled the request. Provider names are
//! not the primary model contract.
//!
//! Resolution order per capability:
//! 1. native Codescribe implementation (always available, workspace sandboxed)
//! 2. Loctree/native semantic enrichment (optional)
//! 3. IntelliJ connector when healthy and project-matched
//! 4. MCP fallback provider
//! 5. typed `capability_unavailable` with recovery guidance

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Canonical capability operations exposed to the agent surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOp {
    FsList,
    FsRead,
    FsSearch,
    FsWrite,
    FsPatch,
    FsMove,
    RepoStatus,
    RepoDiff,
    RepoLog,
    RepoCommit,
    ProcessRun,
    ProcessObserve,
    ProcessStop,
    ProjectBuild,
    ProjectTest,
    ProjectDiagnostics,
    CodeSymbols,
    CodeCalls,
}

impl CapabilityOp {
    /// Canonical dotted name (`fs.read`, `repo.status`).
    ///
    /// This string is the model-facing contract; provider names are not.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FsList => "fs.list",
            Self::FsRead => "fs.read",
            Self::FsSearch => "fs.search",
            Self::FsWrite => "fs.write",
            Self::FsPatch => "fs.patch",
            Self::FsMove => "fs.move",
            Self::RepoStatus => "repo.status",
            Self::RepoDiff => "repo.diff",
            Self::RepoLog => "repo.log",
            Self::RepoCommit => "repo.commit",
            Self::ProcessRun => "process.run",
            Self::ProcessObserve => "process.observe",
            Self::ProcessStop => "process.stop",
            Self::ProjectBuild => "project.build",
            Self::ProjectTest => "project.test",
            Self::ProjectDiagnostics => "project.diagnostics",
            Self::CodeSymbols => "code.symbols",
            Self::CodeCalls => "code.calls",
        }
    }

    /// Native tool name registered with the agent tool registry.
    pub fn native_tool_name(self) -> Option<&'static str> {
        match self {
            Self::FsList => Some("list_directory"),
            Self::FsRead => Some("read_file"),
            Self::FsSearch => Some("search_files"),
            Self::FsWrite => Some("write_file"),
            Self::FsPatch => Some("apply_patch"),
            Self::FsMove => Some("move_path"),
            Self::RepoStatus => Some("git_status"),
            Self::RepoDiff => Some("git_diff"),
            Self::RepoLog => Some("git_log"),
            Self::RepoCommit => Some("git_commit"),
            Self::ProcessRun => Some("run_process"),
            Self::ProcessObserve => Some("observe_process"),
            Self::ProcessStop => Some("stop_process"),
            Self::ProjectBuild => Some("project_build"),
            Self::ProjectTest => Some("project_test"),
            Self::ProjectDiagnostics => None,
            Self::CodeSymbols => None,
            Self::CodeCalls => None,
        }
    }

    /// Whether fulfilling this op can change workspace state or spawn a process.
    pub fn is_mutating(self) -> bool {
        matches!(
            self,
            Self::FsWrite
                | Self::FsPatch
                | Self::FsMove
                | Self::RepoCommit
                | Self::ProcessRun
                | Self::ProcessStop
                | Self::ProjectBuild
                | Self::ProjectTest
        )
    }

    /// Every canonical op. This order is the order rows appear in the matrix
    /// built by [`capability_matrix`].
    pub fn all() -> &'static [CapabilityOp] {
        &[
            Self::FsList,
            Self::FsRead,
            Self::FsSearch,
            Self::FsWrite,
            Self::FsPatch,
            Self::FsMove,
            Self::RepoStatus,
            Self::RepoDiff,
            Self::RepoLog,
            Self::RepoCommit,
            Self::ProcessRun,
            Self::ProcessObserve,
            Self::ProcessStop,
            Self::ProjectBuild,
            Self::ProjectTest,
            Self::ProjectDiagnostics,
            Self::CodeSymbols,
            Self::CodeCalls,
        ]
    }

    /// Parse a canonical dotted name back into an op; `None` for anything
    /// this build does not know.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "fs.list" => Some(Self::FsList),
            "fs.read" => Some(Self::FsRead),
            "fs.search" => Some(Self::FsSearch),
            "fs.write" => Some(Self::FsWrite),
            "fs.patch" => Some(Self::FsPatch),
            "fs.move" => Some(Self::FsMove),
            "repo.status" => Some(Self::RepoStatus),
            "repo.diff" => Some(Self::RepoDiff),
            "repo.log" => Some(Self::RepoLog),
            "repo.commit" => Some(Self::RepoCommit),
            "process.run" => Some(Self::ProcessRun),
            "process.observe" => Some(Self::ProcessObserve),
            "process.stop" => Some(Self::ProcessStop),
            "project.build" => Some(Self::ProjectBuild),
            "project.test" => Some(Self::ProjectTest),
            "project.diagnostics" => Some(Self::ProjectDiagnostics),
            "code.symbols" => Some(Self::CodeSymbols),
            "code.calls" => Some(Self::CodeCalls),
            _ => None,
        }
    }
}

/// Who fulfilled (or would fulfill) a capability request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityProvider {
    Native,
    Loctree,
    IntelliJ,
    Mcp,
    Unavailable,
}

impl CapabilityProvider {
    /// Stable lowercase provider token for logs and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Loctree => "loctree",
            Self::IntelliJ => "intellij",
            Self::Mcp => "mcp",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Availability tier shown in the UI capability matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityTier {
    /// Native implementation always present.
    Native,
    /// Native + optional enrichment (Loctree / healthy matching IntelliJ).
    Enhanced,
    /// No provider can fulfill this operation.
    Unavailable,
}

impl CapabilityTier {
    /// Stable lowercase tier token for logs and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Enhanced => "enhanced",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// One row of the capability matrix, shaped for JSON and UI consumers.
///
/// Carries the op as a dotted string, unlike [`CapabilityResolution`], which
/// keeps the typed [`CapabilityOp`].
pub struct CapabilityStatus {
    pub op: String,
    pub tier: CapabilityTier,
    pub provider: CapabilityProvider,
    pub native_tool: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Outcome of resolving one op against current connector health.
///
/// `provider` records who is *preferred*. Where a native tool exists it stays
/// the fulfilment surface and enrichment is advisory, so `native_tool` can be
/// set even when `provider` names an enricher.
pub struct CapabilityResolution {
    pub op: CapabilityOp,
    pub provider: CapabilityProvider,
    pub native_tool: Option<&'static str>,
    pub reason: String,
}

/// Live connector health inputs used when ranking enrichment providers.
#[derive(Debug, Clone, Default)]
pub struct ConnectorHealth {
    /// Loctree MCP (or native loct) is reachable.
    pub loctree_healthy: bool,
    /// IntelliJ connector is up.
    pub intellij_healthy: bool,
    /// IntelliJ session project path (when known).
    pub intellij_project_path: Option<String>,
    /// Active agent workspace roots (absolute).
    pub workspace_roots: Vec<String>,
    /// MCP servers that advertise a tool covering a given op (by server name).
    pub mcp_servers_with_tools: Vec<String>,
}

impl ConnectorHealth {
    /// True when IntelliJ is healthy AND its project path is under a workspace root.
    pub fn intellij_project_matched(&self) -> bool {
        if !self.intellij_healthy {
            return false;
        }
        let Some(project) = self.intellij_project_path.as_deref() else {
            return false;
        };
        let project = project.trim();
        if project.is_empty() {
            return false;
        }
        self.workspace_roots.iter().any(|root| {
            let root = root.trim_end_matches('/');
            project == root || project.starts_with(&format!("{root}/"))
        })
    }
}

/// Resolve which provider fulfills `op` without changing the operation semantics.
pub fn resolve(op: CapabilityOp, health: &ConnectorHealth) -> CapabilityResolution {
    if let Some(tool) = op.native_tool_name() {
        // Optional enrichment does not replace the native contract.
        let (provider, reason) =
            if matches!(op, CapabilityOp::CodeSymbols | CapabilityOp::CodeCalls) {
                // Native has no semantic/debug implementation for these.
                return unavailable(
                    op,
                    "Semantic code graph is optional and not available natively",
                );
            } else if matches!(
                op,
                CapabilityOp::FsSearch
                    | CapabilityOp::ProjectDiagnostics
                    | CapabilityOp::CodeSymbols
            ) && health.loctree_healthy
            {
                (
                    CapabilityProvider::Loctree,
                    "Native available; Loctree enrichment preferred when healthy".to_string(),
                )
            } else if health.intellij_project_matched()
                && matches!(
                    op,
                    CapabilityOp::FsSearch
                        | CapabilityOp::ProjectDiagnostics
                        | CapabilityOp::CodeSymbols
                        | CapabilityOp::CodeCalls
                        | CapabilityOp::ProjectBuild
                        | CapabilityOp::ProjectTest
                )
            {
                (
                    CapabilityProvider::IntelliJ,
                    "Native available; IntelliJ enrichment when project-matched".to_string(),
                )
            } else if health.intellij_healthy && !health.intellij_project_matched() {
                (
                    CapabilityProvider::Native,
                    "IntelliJ connected but project path does not match workspace roots — bypassed"
                        .to_string(),
                )
            } else {
                (
                    CapabilityProvider::Native,
                    "Native Codescribe implementation".to_string(),
                )
            };
        // When enrichment is "preferred", native remains the fulfillment surface
        // for V1 semantics — enrichment is advisory. Provider records preference.
        let provider = if matches!(
            provider,
            CapabilityProvider::Loctree | CapabilityProvider::IntelliJ
        ) {
            // Still execute via native tool; record preferred enricher for matrix.
            provider
        } else {
            CapabilityProvider::Native
        };
        return CapabilityResolution {
            op,
            provider: if provider == CapabilityProvider::Native {
                CapabilityProvider::Native
            } else {
                // Matrix "enhanced" but tool is still native name.
                provider
            },
            native_tool: Some(tool),
            reason,
        };
    }

    // No native tool — try MCP, else unavailable.
    if !health.mcp_servers_with_tools.is_empty() {
        return CapabilityResolution {
            op,
            provider: CapabilityProvider::Mcp,
            native_tool: None,
            reason: format!(
                "No native tool; MCP fallback via {}",
                health.mcp_servers_with_tools.join(", ")
            ),
        };
    }

    unavailable(
        op,
        "No native implementation and no healthy MCP provider for this capability",
    )
}

/// Build a resolution no provider can serve, carrying `reason` forward as the
/// recovery guidance shown to the caller.
fn unavailable(op: CapabilityOp, reason: impl Into<String>) -> CapabilityResolution {
    CapabilityResolution {
        op,
        provider: CapabilityProvider::Unavailable,
        native_tool: None,
        reason: reason.into(),
    }
}

/// Build the full capability matrix for Settings / readiness UI.
pub fn capability_matrix(health: &ConnectorHealth) -> Vec<CapabilityStatus> {
    CapabilityOp::all()
        .iter()
        .map(|op| {
            let resolution = resolve(*op, health);
            let tier = match resolution.provider {
                CapabilityProvider::Native => CapabilityTier::Native,
                CapabilityProvider::Loctree | CapabilityProvider::IntelliJ => {
                    CapabilityTier::Enhanced
                }
                CapabilityProvider::Mcp => CapabilityTier::Enhanced,
                CapabilityProvider::Unavailable => CapabilityTier::Unavailable,
            };
            // Native tools always present for ops that have them → at least Native tier
            // even when enrichment is preferred.
            let tier = if resolution.native_tool.is_some() && tier == CapabilityTier::Unavailable {
                CapabilityTier::Native
            } else if resolution.native_tool.is_some()
                && matches!(
                    resolution.provider,
                    CapabilityProvider::Loctree | CapabilityProvider::IntelliJ
                )
            {
                CapabilityTier::Enhanced
            } else if resolution.native_tool.is_some() {
                CapabilityTier::Native
            } else {
                tier
            };
            CapabilityStatus {
                op: op.as_str().to_string(),
                tier,
                provider: resolution.provider,
                native_tool: resolution.native_tool.map(str::to_string),
                reason: resolution.reason,
            }
        })
        .collect()
}

/// Schema-backed agent capability preferences (not a copied list of every connector tool).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentCapabilityPreferences {
    /// When true, IntelliJ enrichment is considered when healthy + project-matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefer_intellij_enrichment: Option<bool>,
    /// When true, Loctree semantic tools are preferred for search/symbols.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefer_loctree_enrichment: Option<bool>,
    /// Explicit deny list of canonical ops (never offered).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_ops: Vec<String>,
}

impl AgentCapabilityPreferences {
    /// Whether the user has denied this op. Matches both the canonical dotted
    /// name and any spelling that [`CapabilityOp::parse`] maps to the same op.
    pub fn is_op_disabled(&self, op: CapabilityOp) -> bool {
        self.disabled_ops
            .iter()
            .any(|name| name == op.as_str() || CapabilityOp::parse(name) == Some(op))
    }
}

/// Compact matrix keyed by op for JSON consumers.
pub fn matrix_map(health: &ConnectorHealth) -> BTreeMap<String, CapabilityStatus> {
    capability_matrix(health)
        .into_iter()
        .map(|status| (status.op.clone(), status))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ops_resolve_without_connectors() {
        let health = ConnectorHealth::default();
        let status = resolve(CapabilityOp::FsList, &health);
        assert_eq!(status.provider, CapabilityProvider::Native);
        assert_eq!(status.native_tool, Some("list_directory"));

        let commit = resolve(CapabilityOp::RepoCommit, &health);
        assert_eq!(commit.provider, CapabilityProvider::Native);
        assert!(commit.op.is_mutating());
    }

    #[test]
    fn stale_intellij_project_is_bypassed() {
        let health = ConnectorHealth {
            intellij_healthy: true,
            intellij_project_path: Some("/other/project".into()),
            workspace_roots: vec!["/Users/me/Git/codescribe".into()],
            ..Default::default()
        };
        let status = resolve(CapabilityOp::RepoStatus, &health);
        assert_eq!(status.provider, CapabilityProvider::Native);
        assert!(status.reason.contains("bypassed") || status.reason.contains("Native"));
        assert!(!health.intellij_project_matched());
    }

    #[test]
    fn matched_intellij_project_enhances_build() {
        let health = ConnectorHealth {
            intellij_healthy: true,
            intellij_project_path: Some("/Users/me/Git/codescribe".into()),
            workspace_roots: vec!["/Users/me/Git/codescribe".into()],
            ..Default::default()
        };
        assert!(health.intellij_project_matched());
        let status = resolve(CapabilityOp::ProjectBuild, &health);
        assert_eq!(status.provider, CapabilityProvider::IntelliJ);
        assert_eq!(status.native_tool, Some("project_build"));
    }

    #[test]
    fn optional_semantics_unavailable_without_providers() {
        let health = ConnectorHealth::default();
        let status = resolve(CapabilityOp::CodeSymbols, &health);
        assert_eq!(status.provider, CapabilityProvider::Unavailable);
        assert!(status.native_tool.is_none());
    }

    #[test]
    fn matrix_covers_all_ops() {
        let matrix = capability_matrix(&ConnectorHealth::default());
        assert_eq!(matrix.len(), CapabilityOp::all().len());
        assert!(
            matrix
                .iter()
                .any(|s| s.op == "fs.read" && s.native_tool.is_some())
        );
        assert!(
            matrix
                .iter()
                .any(|s| s.op == "code.symbols" && s.tier == CapabilityTier::Unavailable)
        );
    }
}
