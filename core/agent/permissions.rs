//! Agent tool permission policy — allow / ask / deny gateway.
//!
//! Resolution order (first match wins):
//! 1. Per-thread override (session-local, not durable)
//! 2. Per-tool override (`server:upstream` or `native:name`)
//! 3. Per-MCP-server override
//! 4. Risk-class defaults (`read_only` → allow, side-effectful → ask)
//! 5. Global default (fallback when risk is unknown)
//!
//! Hard rules outside this table:
//! - Unregistered tools → deny
//! - `ToolRisk::Destructive` MCP tools → deny (no override can lift)
//!
//! Durable preferences live in `settings.json` under `agent.permissions`.
//! "Always allow" from the approval card writes the tool key to that map
//! (and dual-writes `tool_grants.json` so existing revoke UI keeps working).
//! One identity key: [`tool_identity`] / [`crate::agent::tool_grants::grant_key`].

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::registry::{ToolOrigin, ToolRisk};
use super::tool_grants;

/// Tri-state operator preference for a tool / server / global default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// Run without prompting.
    Allow,
    /// Prompt the operator for approval on every call.
    #[default]
    Ask,
    /// Refuse the call outright.
    Deny,
}

impl PermissionLevel {
    /// Wire/JSON spelling of this level (`allow` | `ask` | `deny`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    /// Parse a level from its wire spelling (case- and whitespace-insensitive).
    /// Errors rather than defaulting — an unrecognized level must not silently
    /// become `Ask`.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "allow" => Ok(Self::Allow),
            "ask" => Ok(Self::Ask),
            "deny" => Ok(Self::Deny),
            other => bail!("unknown permission level '{other}' (want allow|ask|deny)"),
        }
    }
}

/// Durable permission section stored under `agent.permissions` in settings.json.
///
/// Migration defaults for existing users (absent section):
/// - read-only tools → `allow`
/// - side-effectful tools → `ask`
/// - global fallback → `ask`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentPermissions {
    /// Fallback when risk class has no dedicated default.
    pub default: PermissionLevel,
    /// Default for `ToolRisk::ReadOnly` when no more-specific rule matches.
    pub read_only_default: PermissionLevel,
    /// Default for mutating / network / process / unknown side effects.
    pub side_effect_default: PermissionLevel,
    /// Per-tool identity → level. Keys use [`tool_identity`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, PermissionLevel>,
    /// Per-MCP-server name (case-insensitive) → level.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, PermissionLevel>,
}

impl Default for AgentPermissions {
    fn default() -> Self {
        Self {
            default: PermissionLevel::Ask,
            read_only_default: PermissionLevel::Allow,
            side_effect_default: PermissionLevel::Ask,
            tools: BTreeMap::new(),
            servers: BTreeMap::new(),
        }
    }
}

impl AgentPermissions {
    /// Load durable policy from settings.json. Missing section → migration defaults.
    pub fn load() -> Self {
        crate::config::UserSettings::load()
            .agent_permissions
            .unwrap_or_default()
    }

    /// Persist the full policy into settings.json (preserves other settings).
    pub fn save(&self) -> Result<()> {
        let mut settings = crate::config::UserSettings::load();
        settings.agent_permissions = Some(self.clone());
        settings
            .save()
            .context("Failed to persist agent.permissions to settings.json")
    }

    /// Merge legacy `tool_grants.json` always-allow keys as tool-level `Allow`.
    /// Existing grants keep working; settings remain the product source of truth
    /// for new remembers.
    pub fn with_legacy_grants(mut self, granted: impl IntoIterator<Item = String>) -> Self {
        for key in granted {
            self.tools.entry(key).or_insert(PermissionLevel::Allow);
        }
        self
    }

    /// Resolve the effective level for one tool call.
    ///
    /// Precedence: thread > tool > server > risk default > global default.
    pub fn resolve(
        &self,
        identity: &str,
        server: Option<&str>,
        risk: ToolRisk,
        thread_override: Option<PermissionLevel>,
    ) -> PermissionLevel {
        if let Some(level) = thread_override {
            return level;
        }
        if let Some(level) = self.tools.get(identity) {
            return *level;
        }
        if let Some(server) = server {
            let server_key = server.to_ascii_lowercase();
            let server_level = self.servers.get(&server_key).copied().or_else(|| {
                // Tolerate mixed-case keys written by hand.
                self.servers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(server))
                    .map(|(_, level)| *level)
            });
            if let Some(level) = server_level {
                // A blanket server Allow covers the tools the operator saw when
                // they granted it — not a tool the server grows later. Unknown
                // risk means "we have never classified this", so it still asks
                // (review P2-16). Deny/Ask stay authoritative.
                return match (level, risk) {
                    (PermissionLevel::Allow, ToolRisk::Unknown) => PermissionLevel::Ask,
                    _ => level,
                };
            }
        }
        match risk {
            ToolRisk::ReadOnly => self.read_only_default,
            ToolRisk::Destructive => PermissionLevel::Deny,
            ToolRisk::Mutating | ToolRisk::ProcessControl | ToolRisk::Network => {
                self.side_effect_default
            }
            // Unknown falls to the product-facing global default so operators
            // can tighten new tools without reclassifying risk tables.
            ToolRisk::Unknown => self.default,
        }
    }

    /// Convenience: resolve using origin + registered tool name.
    pub fn resolve_for_origin(
        &self,
        tool_name: &str,
        origin: &ToolOrigin,
        risk: ToolRisk,
        thread_overrides: &HashMap<String, PermissionLevel>,
    ) -> PermissionLevel {
        let identity = tool_identity(origin, tool_name);
        let server = match origin {
            ToolOrigin::Mcp { server, .. } => Some(server.as_str()),
            ToolOrigin::Native => None,
        };
        let thread = thread_overrides.get(&identity).copied();
        self.resolve(&identity, server, risk, thread)
    }

    /// Record "always allow" for one MCP tool into settings.json.
    /// Same identity the dispatcher checks — never assemble the key elsewhere.
    pub fn remember_allow(server: &str, upstream_tool: &str) -> Result<()> {
        let identity = tool_grants::grant_key(server, upstream_tool);
        let mut policy = Self::load();
        policy.tools.insert(identity, PermissionLevel::Allow);
        policy.save()?;
        // Dual-write so list/revoke grant surfaces stay truthful until fully
        // migrated to the permissions panel.
        tool_grants::grant(server, upstream_tool)?;
        Ok(())
    }

    /// Set a durable tool-level preference (Settings panel).
    pub fn set_tool_level(identity: &str, level: PermissionLevel) -> Result<()> {
        let identity = identity.trim();
        if identity.is_empty() {
            bail!("tool identity must not be empty");
        }
        let mut policy = Self::load();
        policy.tools.insert(identity.to_string(), level);
        policy.save()?;
        // Keep tool_grants.json in sync for allow/revoke of MCP keys.
        if let Some((server, tool)) = identity.split_once(':')
            && server != "native"
        {
            match level {
                PermissionLevel::Allow => {
                    let _ = tool_grants::grant(server, tool);
                }
                PermissionLevel::Ask | PermissionLevel::Deny => {
                    let _ = tool_grants::revoke(identity);
                }
            }
        }
        Ok(())
    }

    /// Set a durable per-server preference.
    pub fn set_server_level(server: &str, level: PermissionLevel) -> Result<()> {
        let server = server.trim();
        if server.is_empty() {
            bail!("server name must not be empty");
        }
        let mut policy = Self::load();
        policy.servers.insert(server.to_ascii_lowercase(), level);
        policy.save()
    }

    /// Set the global / risk-class defaults.
    pub fn set_defaults(
        default: PermissionLevel,
        read_only_default: PermissionLevel,
        side_effect_default: PermissionLevel,
    ) -> Result<()> {
        let mut policy = Self::load();
        policy.default = default;
        policy.read_only_default = read_only_default;
        policy.side_effect_default = side_effect_default;
        policy.save()
    }
}

/// Canonical tool identity used by policy maps and grants.
///
/// MCP: `server:upstream_tool` (server lowercased — see [`tool_grants::grant_key`]).
/// Native: `native:<tool_name>` so native and MCP never collide on bare names.
pub fn tool_identity(origin: &ToolOrigin, tool_name: &str) -> String {
    match origin {
        ToolOrigin::Mcp {
            server,
            upstream_tool,
        } => tool_grants::grant_key(server, upstream_tool),
        ToolOrigin::Native => format!("native:{tool_name}"),
    }
}

/// One capability row for the Settings panel / bridge — sourced from the same
/// registry the dispatcher uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCapability {
    /// Registered tool name as the dispatcher exposes it to the model.
    pub name: String,
    /// Canonical policy key — see [`tool_identity`].
    pub identity: String,
    /// `mcp` or `native`.
    pub origin: String,
    /// Owning MCP server, `None` for native tools.
    pub server: Option<String>,
    /// Risk class label used by the risk-default rules.
    pub risk: String,
    /// Level [`AgentPermissions::resolve`] currently returns for this tool.
    pub effective: PermissionLevel,
    /// Whether the tool declares approval-required independently of policy.
    pub requires_approval_flag: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::registry::ToolOrigin;

    fn mcp(server: &str, tool: &str) -> ToolOrigin {
        ToolOrigin::Mcp {
            server: server.to_string(),
            upstream_tool: tool.to_string(),
        }
    }

    #[test]
    fn precedence_thread_over_tool_over_server_over_risk() {
        let mut policy = AgentPermissions {
            side_effect_default: PermissionLevel::Ask,
            read_only_default: PermissionLevel::Allow,
            ..Default::default()
        };
        policy
            .servers
            .insert("desktop-commander".into(), PermissionLevel::Deny);
        let identity = tool_grants::grant_key("desktop-commander", "write_file");
        policy
            .tools
            .insert(identity.clone(), PermissionLevel::Allow);

        // Tool beats server.
        assert_eq!(
            policy.resolve(
                &identity,
                Some("desktop-commander"),
                ToolRisk::Mutating,
                None
            ),
            PermissionLevel::Allow
        );

        // Thread beats tool.
        assert_eq!(
            policy.resolve(
                &identity,
                Some("desktop-commander"),
                ToolRisk::Mutating,
                Some(PermissionLevel::Deny),
            ),
            PermissionLevel::Deny
        );

        // Absent tool → server.
        assert_eq!(
            policy.resolve(
                "desktop-commander:other",
                Some("desktop-commander"),
                ToolRisk::Mutating,
                None,
            ),
            PermissionLevel::Deny
        );

        // Absent tool+server → risk default (read-only allow).
        assert_eq!(
            policy.resolve("native:read_file", None, ToolRisk::ReadOnly, None),
            PermissionLevel::Allow
        );

        // Absent tool+server → risk default (side-effect ask).
        assert_eq!(
            policy.resolve("native:type_text", None, ToolRisk::Mutating, None),
            PermissionLevel::Ask
        );
    }

    #[test]
    fn absent_levels_fall_through_deterministically() {
        let policy = AgentPermissions::default();
        assert_eq!(
            policy.resolve("x:y", Some("x"), ToolRisk::Unknown, None),
            PermissionLevel::Ask
        );
        assert_eq!(
            policy.resolve("x:y", Some("x"), ToolRisk::ReadOnly, None),
            PermissionLevel::Allow
        );
        assert_eq!(
            policy.resolve("x:y", Some("x"), ToolRisk::Destructive, None),
            PermissionLevel::Deny
        );
    }

    #[test]
    fn identity_is_single_key_server_case_insensitive() {
        let a = tool_identity(
            &mcp("Desktop-Commander", "write_file"),
            "mcp__dc__write_file",
        );
        let b = tool_identity(
            &mcp("desktop-commander", "write_file"),
            "mcp__dc__write_file",
        );
        assert_eq!(a, b);
        assert_eq!(a, "desktop-commander:write_file");
        // Cosmetic registered name must not participate in identity.
        let c = tool_identity(&mcp("desktop-commander", "write_file"), "totally_different");
        assert_eq!(a, c);
        // Tool name case is preserved (upstream tools are case-sensitive).
        assert_ne!(
            tool_identity(&mcp("srv", "Tool"), "x"),
            tool_identity(&mcp("srv", "tool"), "x")
        );
        assert_eq!(
            tool_identity(&ToolOrigin::Native, "read_file"),
            "native:read_file"
        );
    }

    #[test]
    fn resolve_for_origin_honours_thread_map() {
        let policy = AgentPermissions::default();
        let origin = mcp("srv", "tool");
        let identity = tool_identity(&origin, "registered");
        let mut thread = HashMap::new();
        thread.insert(identity, PermissionLevel::Deny);
        assert_eq!(
            policy.resolve_for_origin("registered", &origin, ToolRisk::ReadOnly, &thread),
            PermissionLevel::Deny
        );
    }

    #[test]
    fn legacy_grants_merge_as_allow_without_clobbering_deny() {
        let mut policy = AgentPermissions::default();
        let key = tool_grants::grant_key("srv", "tool");
        policy.tools.insert(key.clone(), PermissionLevel::Deny);
        let merged = policy
            .clone()
            .with_legacy_grants([key.clone(), "other:tool".into()]);
        assert_eq!(merged.tools.get(&key), Some(&PermissionLevel::Deny));
        assert_eq!(
            merged.tools.get("other:tool"),
            Some(&PermissionLevel::Allow)
        );
    }
}
