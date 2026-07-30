use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};

use super::permissions::{AgentPermissions, PermissionLevel, ToolCapability, tool_identity};
use super::types::ImageAsset;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolResultContent {
    Text(String),
    Image { data: Vec<u8>, media_type: String },
    ImageAsset(ImageAsset),
    Error(String),
}

pub type ToolFuture = Pin<Box<dyn Future<Output = Vec<ToolResultContent>> + Send>>;
pub type ToolHandler = Box<dyn Fn(serde_json::Value) -> ToolFuture + Send + Sync>;
pub type ToolInputValidator =
    Arc<dyn Fn(&serde_json::Value) -> Result<ToolCallPreview> + Send + Sync>;

pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
    /// Durable allow/ask/deny policy (settings.json `agent.permissions` +
    /// legacy tool_grants). Single choke point for resolution.
    policy: AgentPermissions,
    /// Per-thread tool-identity overrides for the active turn (not durable).
    thread_overrides: HashMap<String, PermissionLevel>,
}

struct RegisteredTool {
    definition: ToolDefinition,
    handler: ToolHandler,
    policy: ToolExecutionPolicy,
    validator: Option<ToolInputValidator>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOrigin {
    Native,
    Mcp {
        server: String,
        upstream_tool: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    ReadOnly,
    Mutating,
    ProcessControl,
    Network,
    Destructive,
    Unknown,
}

impl ToolRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Mutating => "mutating",
            Self::ProcessControl => "process_control",
            Self::Network => "network",
            Self::Destructive => "destructive",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionPolicy {
    pub origin: ToolOrigin,
    pub risk: ToolRisk,
    pub requires_approval: bool,
}

impl ToolExecutionPolicy {
    pub fn native_compatible() -> Self {
        Self {
            origin: ToolOrigin::Native,
            risk: ToolRisk::Unknown,
            requires_approval: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallPreview {
    pub summary: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolApprovalRequest {
    pub call_id: String,
    pub session_id: String,
    pub thread_id: String,
    pub tool: String,
    pub origin: ToolOrigin,
    pub risk: ToolRisk,
    pub summary: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDecision {
    Allow,
    RequireApproval(Box<ToolApprovalRequest>),
    Deny(String),
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            policy: AgentPermissions::default(),
            thread_overrides: HashMap::new(),
        }
    }

    /// Install the full permission policy (settings + legacy grants).
    pub fn set_policy(&mut self, policy: AgentPermissions) {
        self.policy = policy;
    }

    /// Install the operator's persisted "always allow" grant keys
    /// (see [`crate::agent::tool_grants`]). Merged into policy as tool-level
    /// `Allow` without clobbering an explicit Deny in settings.
    pub fn set_granted(&mut self, granted: std::collections::HashSet<String>) {
        self.policy = std::mem::take(&mut self.policy).with_legacy_grants(granted);
    }

    /// Replace per-thread overrides for the active session turn.
    pub fn set_thread_overrides(&mut self, overrides: HashMap<String, PermissionLevel>) {
        self.thread_overrides = overrides;
    }

    /// Set one per-thread override (agent UI). Identity must be
    /// [`tool_identity`] / grant key form.
    pub fn set_thread_override(&mut self, identity: String, level: PermissionLevel) {
        self.thread_overrides.insert(identity, level);
    }

    pub fn clear_thread_overrides(&mut self) {
        self.thread_overrides.clear();
    }

    pub fn permission_policy(&self) -> &AgentPermissions {
        &self.policy
    }

    /// Capability list from the SAME registry the dispatcher uses.
    pub fn capabilities(&self) -> Vec<ToolCapability> {
        let mut caps: Vec<ToolCapability> = self
            .tools
            .iter()
            .map(|(name, tool)| {
                let identity = tool_identity(&tool.policy.origin, name);
                let server = match &tool.policy.origin {
                    ToolOrigin::Mcp { server, .. } => Some(server.clone()),
                    ToolOrigin::Native => None,
                };
                let origin = match &tool.policy.origin {
                    ToolOrigin::Native => "native".to_string(),
                    ToolOrigin::Mcp { server, .. } => format!("mcp:{server}"),
                };
                let effective = self.policy.resolve_for_origin(
                    name,
                    &tool.policy.origin,
                    tool.policy.risk,
                    &self.thread_overrides,
                );
                ToolCapability {
                    name: name.clone(),
                    identity,
                    origin,
                    server,
                    risk: tool.policy.risk.as_str().to_string(),
                    effective,
                    requires_approval_flag: tool.policy.requires_approval,
                }
            })
            .collect();
        caps.sort_by(|a, b| a.identity.cmp(&b.identity));
        caps
    }

    pub fn register(&mut self, definition: ToolDefinition, handler: ToolHandler) -> Result<()> {
        self.register_with_policy(
            definition,
            handler,
            ToolExecutionPolicy::native_compatible(),
            None,
        )
    }

    pub fn register_with_policy(
        &mut self,
        definition: ToolDefinition,
        handler: ToolHandler,
        policy: ToolExecutionPolicy,
        validator: Option<ToolInputValidator>,
    ) -> Result<()> {
        let name = definition.name.clone();
        if self.tools.contains_key(&name) {
            anyhow::bail!("Tool '{}' is already registered", name);
        }
        self.tools.insert(
            name,
            RegisteredTool {
                definition,
                handler,
                policy,
                validator,
            },
        );
        Ok(())
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    /// Per-tool execution metadata (origin/risk/requires_approval flag).
    pub fn policy(&self, name: &str) -> Option<&ToolExecutionPolicy> {
        self.tools.get(name).map(|tool| &tool.policy)
    }

    /// Single choke point: resolve allow / ask / deny before any side effect.
    pub fn decide(
        &self,
        name: &str,
        input: &serde_json::Value,
        call_id: &str,
        session_id: &str,
        thread_id: &str,
    ) -> ToolDecision {
        let Some(tool) = self.tools.get(name) else {
            return ToolDecision::Deny(format!("Tool '{name}' is not registered"));
        };
        let preview = match &tool.validator {
            Some(validate) => match validate(input) {
                Ok(preview) => preview,
                Err(error) => return ToolDecision::Deny(error.to_string()),
            },
            None => ToolCallPreview::default(),
        };

        // Destructive stays a hard deny — no approval, grant, or settings
        // override can lift it (defense in depth on top of risk defaults).
        if matches!(tool.policy.origin, ToolOrigin::Mcp { .. })
            && tool.policy.risk == ToolRisk::Destructive
        {
            return ToolDecision::Deny(format!(
                "External tool '{name}' is denied by policy ({})",
                tool.policy.risk.as_str()
            ));
        }

        let identity = tool_identity(&tool.policy.origin, name);
        let level = if self.has_explicit_rule(&identity, &tool.policy.origin) {
            // Explicit operator preference (thread / tool / server / grant).
            self.policy.resolve_for_origin(
                name,
                &tool.policy.origin,
                tool.policy.risk,
                &self.thread_overrides,
            )
        } else {
            // Migration defaults when no preference is stored:
            // - requires_approval flag or MCP Unknown risk → ask
            // - read-only (no gate flag) → allow
            // - native tools without gate flag → allow (prior runtime contract)
            // - other side-effectful classifications → ask
            match (
                tool.policy.requires_approval,
                tool.policy.risk,
                &tool.policy.origin,
            ) {
                (true, _, _) => PermissionLevel::Ask,
                (false, ToolRisk::Unknown, ToolOrigin::Mcp { .. }) => PermissionLevel::Ask,
                (false, ToolRisk::ReadOnly, _) => PermissionLevel::Allow,
                (false, ToolRisk::Mutating | ToolRisk::ProcessControl | ToolRisk::Network, _) => {
                    PermissionLevel::Ask
                }
                (false, ToolRisk::Unknown, ToolOrigin::Native) => PermissionLevel::Allow,
                (false, ToolRisk::Destructive, _) => PermissionLevel::Deny,
            }
        };

        match level {
            PermissionLevel::Allow => ToolDecision::Allow,
            PermissionLevel::Deny => {
                ToolDecision::Deny(format!("Tool '{name}' is denied by operator policy"))
            }
            PermissionLevel::Ask => ToolDecision::RequireApproval(Box::new(ToolApprovalRequest {
                call_id: call_id.to_string(),
                session_id: session_id.to_string(),
                thread_id: thread_id.to_string(),
                tool: name.to_string(),
                origin: tool.policy.origin.clone(),
                risk: tool.policy.risk,
                summary: preview.summary,
                command: preview.command,
                cwd: preview.cwd,
                paths: preview.paths,
            })),
        }
    }

    fn has_explicit_rule(&self, identity: &str, origin: &ToolOrigin) -> bool {
        if self.thread_overrides.contains_key(identity) {
            return true;
        }
        if self.policy.tools.contains_key(identity) {
            return true;
        }
        if let ToolOrigin::Mcp { server, .. } = origin {
            let key = server.to_ascii_lowercase();
            if self.policy.servers.contains_key(&key)
                || self
                    .policy
                    .servers
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case(server))
            {
                return true;
            }
        }
        false
    }

    pub async fn dispatch(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<Vec<ToolResultContent>> {
        let tool = self
            .tools
            .get(name)
            .with_context(|| format!("Tool '{}' is not registered", name))?;
        Ok((tool.handler)(input).await)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use std::sync::Arc;

    use super::super::permissions::{AgentPermissions, PermissionLevel};
    use super::{
        ToolDecision, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolRegistry,
        ToolResultContent, ToolRisk,
    };

    #[tokio::test]
    async fn dispatches_registered_tool() {
        let mut registry = ToolRegistry::new();
        registry
            .register(
                ToolDefinition {
                    name: "echo_name".to_string(),
                    description: "Echoes the provided name".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": { "name": { "type": "string" } }
                    }),
                },
                Box::new(|input| {
                    Box::pin(async move {
                        let name = input
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown");
                        vec![ToolResultContent::Text(format!("hello {name}"))]
                    })
                }),
            )
            .expect("tool registration should succeed");

        let result = registry
            .dispatch("echo_name", json!({ "name": "vetcoders" }))
            .await
            .expect("tool dispatch should succeed");

        assert_eq!(
            result,
            vec![ToolResultContent::Text("hello vetcoders".to_string())]
        );
    }

    fn register_external(registry: &mut ToolRegistry, name: &str, risk: ToolRisk) {
        registry
            .register_with_policy(
                ToolDefinition {
                    name: name.to_string(),
                    description: "gated external tool".to_string(),
                    input_schema: json!({"type": "object"}),
                },
                Box::new(|_| Box::pin(async { Vec::new() })),
                ToolExecutionPolicy {
                    origin: ToolOrigin::Mcp {
                        server: "desktop-commander".to_string(),
                        upstream_tool: name.to_string(),
                    },
                    risk,
                    requires_approval: false,
                },
                None,
            )
            .expect("register external tool");
    }

    #[test]
    fn external_destructive_tool_is_denied_even_when_granted() {
        let mut registry = ToolRegistry::new();
        register_external(&mut registry, "wipe_disk", ToolRisk::Destructive);
        registry.set_granted(std::collections::HashSet::from([
            crate::agent::tool_grants::grant_key("desktop-commander", "wipe_disk"),
        ]));
        assert!(matches!(
            registry.decide("wipe_disk", &json!({}), "call", "session", "thread"),
            ToolDecision::Deny(_)
        ));
    }

    #[test]
    fn external_unknown_tool_asks_instead_of_denying() {
        let mut registry = ToolRegistry::new();
        register_external(&mut registry, "future_tool", ToolRisk::Unknown);
        let ToolDecision::RequireApproval(request) =
            registry.decide("future_tool", &json!({}), "call", "session", "thread")
        else {
            panic!("unknown external tool must ask, not deny or allow");
        };
        assert_eq!(request.risk, ToolRisk::Unknown);
        assert_eq!(request.tool, "future_tool");
    }

    #[test]
    fn granted_external_tool_skips_the_approval_gate() {
        let mut registry = ToolRegistry::new();
        register_external(&mut registry, "future_tool", ToolRisk::Unknown);
        registry.set_granted(std::collections::HashSet::from([
            crate::agent::tool_grants::grant_key("Desktop-Commander", "future_tool"),
        ]));
        assert_eq!(
            registry.decide("future_tool", &json!({}), "call", "session", "thread"),
            ToolDecision::Allow
        );
        // A grant for a DIFFERENT tool changes nothing — rebuild policy so
        // the previous grant is not left merged in (set_granted is additive).
        registry.set_policy(AgentPermissions::default().with_legacy_grants([
            crate::agent::tool_grants::grant_key("desktop-commander", "other_tool"),
        ]));
        assert!(matches!(
            registry.decide("future_tool", &json!({}), "call", "session", "thread"),
            ToolDecision::RequireApproval(_)
        ));
    }

    #[test]
    fn deny_blocks_at_decide_without_dispatch_side_effect() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_handler = Arc::clone(&hits);
        let mut registry = ToolRegistry::new();
        registry
            .register_with_policy(
                ToolDefinition {
                    name: "boom".into(),
                    description: "side effect".into(),
                    input_schema: json!({"type": "object"}),
                },
                Box::new(move |_| {
                    hits_handler.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Vec::new() })
                }),
                ToolExecutionPolicy {
                    origin: ToolOrigin::Mcp {
                        server: "srv".into(),
                        upstream_tool: "boom".into(),
                    },
                    risk: ToolRisk::Mutating,
                    requires_approval: true,
                },
                None,
            )
            .expect("register");
        let mut policy = AgentPermissions::default();
        policy
            .tools
            .insert("srv:boom".into(), PermissionLevel::Deny);
        registry.set_policy(policy);
        assert!(matches!(
            registry.decide("boom", &json!({}), "c", "s", "t"),
            ToolDecision::Deny(_)
        ));
        // decide must never invoke the handler — only dispatch does.
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn thread_override_outranks_tool_and_server() {
        let mut registry = ToolRegistry::new();
        register_external(&mut registry, "write_file", ToolRisk::Mutating);
        let mut policy = AgentPermissions::default();
        policy
            .servers
            .insert("desktop-commander".into(), PermissionLevel::Deny);
        policy.tools.insert(
            crate::agent::tool_grants::grant_key("desktop-commander", "write_file"),
            PermissionLevel::Allow,
        );
        registry.set_policy(policy);
        assert_eq!(
            registry.decide("write_file", &json!({}), "c", "s", "t"),
            ToolDecision::Allow
        );
        registry.set_thread_override(
            crate::agent::tool_grants::grant_key("desktop-commander", "write_file"),
            PermissionLevel::Deny,
        );
        assert!(matches!(
            registry.decide("write_file", &json!({}), "c", "s", "t"),
            ToolDecision::Deny(_)
        ));
    }

    #[test]
    fn capability_list_is_registry_truth() {
        let mut registry = ToolRegistry::new();
        register_external(&mut registry, "read_resource", ToolRisk::ReadOnly);
        let caps = registry.capabilities();
        assert!(
            caps.iter()
                .any(|c| c.identity == "desktop-commander:read_resource")
        );
        assert!(!caps.iter().any(|c| c.name == "not_registered"));
        // Unregistered cannot appear — registry is the sole source.
        assert!(registry.policy("not_registered").is_none());
    }

    #[test]
    fn always_allow_identity_miss_does_not_skip_gate() {
        let mut registry = ToolRegistry::new();
        register_external(&mut registry, "write_file", ToolRisk::Mutating);
        // Cosmetic mismatch: wrong upstream name must not grant.
        registry.set_granted(std::collections::HashSet::from([
            crate::agent::tool_grants::grant_key("desktop-commander", "writeFile"),
        ]));
        assert!(matches!(
            registry.decide("write_file", &json!({}), "c", "s", "t"),
            ToolDecision::RequireApproval(_)
        ));
        // Correct identity key allows.
        registry.set_policy(AgentPermissions::default().with_legacy_grants([
            crate::agent::tool_grants::grant_key("Desktop-Commander", "write_file"),
        ]));
        assert_eq!(
            registry.decide("write_file", &json!({}), "c", "s", "t"),
            ToolDecision::Allow
        );
    }

    #[test]
    fn read_only_allows_and_mutation_binds_approval_identity() {
        let mut registry = ToolRegistry::new();
        for (name, risk, requires_approval) in [
            ("read", ToolRisk::ReadOnly, false),
            ("write", ToolRisk::Mutating, true),
        ] {
            registry
                .register_with_policy(
                    ToolDefinition {
                        name: name.to_string(),
                        description: name.to_string(),
                        input_schema: json!({"type": "object"}),
                    },
                    Box::new(|_| Box::pin(async { Vec::new() })),
                    ToolExecutionPolicy {
                        origin: ToolOrigin::Mcp {
                            server: "desktop-commander".to_string(),
                            upstream_tool: name.to_string(),
                        },
                        risk,
                        requires_approval,
                    },
                    None,
                )
                .expect("register classified tool");
        }
        assert_eq!(
            registry.decide("read", &json!({}), "call-read", "session-a", "thread-a"),
            ToolDecision::Allow
        );
        let ToolDecision::RequireApproval(request) =
            registry.decide("write", &json!({}), "call-write", "session-a", "thread-a")
        else {
            panic!("mutation must require approval");
        };
        assert_eq!(request.call_id, "call-write");
        assert_eq!(request.session_id, "session-a");
        assert_eq!(request.thread_id, "thread-a");
    }
}
