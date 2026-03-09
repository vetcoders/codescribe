use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Result};

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
    Error(String),
}

pub type ToolFuture = Pin<Box<dyn Future<Output = Vec<ToolResultContent>> + Send>>;
pub type ToolHandler = Box<dyn Fn(serde_json::Value) -> ToolFuture + Send + Sync>;

pub struct ToolRegistry {
    tools: HashMap<String, (ToolDefinition, ToolHandler)>,
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
        }
    }

    pub fn register(&mut self, definition: ToolDefinition, handler: ToolHandler) -> Result<()> {
        let name = definition.name.clone();
        if self.tools.contains_key(&name) {
            anyhow::bail!("Tool '{}' is already registered", name);
        }
        self.tools.insert(name, (definition, handler));
        Ok(())
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|(definition, _)| definition.clone())
            .collect()
    }

    pub async fn dispatch(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<Vec<ToolResultContent>> {
        let (_, handler) = self
            .tools
            .get(name)
            .with_context(|| format!("Tool '{}' is not registered", name))?;
        Ok((handler)(input).await)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ToolDefinition, ToolRegistry, ToolResultContent};

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
}
