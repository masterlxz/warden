use async_trait::async_trait;
use serde_json::Value;

pub mod delegate;
pub mod file_tools;
pub mod shell;
pub mod web_search;

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// A capability the orchestrator can invoke (file access, shell, web search, browser).
/// MCP-style: name + JSON schema for params, executed against a JSON value.
#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn call(&self, args: Value) -> anyhow::Result<Value>;
}
