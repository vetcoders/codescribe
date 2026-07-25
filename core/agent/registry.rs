use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};

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
    /// Persisted "always allow" grant keys (`server:upstream_tool`), loaded
    /// once per session by the embedder. Never consulted for Destructive risk.
    granted: std::collections::HashSet<String>,
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
            granted: std::collections::HashSet::new(),
        }
    }

    /// Install the operator's persisted "always allow" grant keys
    /// (see [`crate::agent::tool_grants`]). Loaded once at session build;
    /// an empty set means every gated tool asks.
    pub fn set_granted(&mut self, granted: std::collections::HashSet<String>) {
        self.granted = granted;
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

    pub fn policy(&self, name: &str) -> Option<&ToolExecutionPolicy> {
        self.tools.get(name).map(|tool| &tool.policy)
    }

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

        // Destructive stays a hard deny — no approval and no grant can lift it.
        if matches!(tool.policy.origin, ToolOrigin::Mcp { .. })
            && tool.policy.risk == ToolRisk::Destructive
        {
            return ToolDecision::Deny(format!(
                "External tool '{name}' is denied by policy ({})",
                tool.policy.risk.as_str()
            ));
        }

        // Unknown risk asks instead of denying: a freshly added upstream tool
        // is unusable until a human looks at it, but a human CAN look at it.
        let needs_gate = tool.policy.requires_approval
            || (matches!(tool.policy.origin, ToolOrigin::Mcp { .. })
                && tool.policy.risk == ToolRisk::Unknown);

        if needs_gate {
            if let ToolOrigin::Mcp {
                server,
                upstream_tool,
            } = &tool.policy.origin
                && self
                    .granted
                    .contains(&crate::agent::tool_grants::grant_key(server, upstream_tool))
            {
                return ToolDecision::Allow;
            }
            return ToolDecision::RequireApproval(Box::new(ToolApprovalRequest {
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
            }));
        }

        ToolDecision::Allow
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
        // A grant for a DIFFERENT tool changes nothing.
        registry.set_granted(std::collections::HashSet::from([
            crate::agent::tool_grants::grant_key("desktop-commander", "other_tool"),
        ]));
        assert!(matches!(
            registry.decide("future_tool", &json!({}), "call", "session", "thread"),
            ToolDecision::RequireApproval(_)
        ));
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
