use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

pub mod delegate;
pub mod file_tools;
pub mod mcp;
pub mod mcp_oauth;
pub mod shell;

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

/// A source of tools that isn't known until runtime — unlike `Tool`, which is a single
/// fixed capability compiled into the binary. The motivating case is an MCP server: connecting
/// to one doesn't give you a fixed, named tool, it gives you whatever set of tools that server
/// happens to advertise (`tools/list`), discovered only after the connection is made. A
/// `ToolProvider` bridges that gap so the bootstrap/registration code can treat "one hardcoded
/// tool" and "N tools from an external server" the same way: call `tools()`, register whatever
/// comes back.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    async fn tools(&self) -> anyhow::Result<Vec<Arc<dyn Tool>>>;
}
