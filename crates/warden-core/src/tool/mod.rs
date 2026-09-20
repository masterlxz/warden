use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod delegate;
pub mod delegate_to_agent;
pub mod document;
pub mod file_tools;
pub mod mcp;
pub mod mcp_oauth;
pub mod shell;
pub mod skill_tools;
pub mod ssh;

/// `Serialize`/`Deserialize` let this be reused directly as the wire shape for a client-advertised
/// tool (`warden-server`'s `ClientMessage::Hello.tools`, Fase 7.4) — no parallel wire struct needed.
/// `PartialEq` so `ClientMessage` (which derives it for its own round-trip tests) can too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// A copy of this tool scoped to `agent` (`None` = no agent, e.g. a plain Telegram chat), or
    /// `None` when the tool doesn't care which agent is speaking. Called by
    /// `Orchestrator::with_agent`; a tool that restricts what an agent may reach (`ssh_exec`)
    /// overrides it, and must keep enough state to be re-scoped again later.
    fn scoped_to_agent(&self, _agent: Option<&str>) -> Option<Arc<dyn Tool>> {
        None
    }

    /// Whether the model should be offered this tool right now. The orchestrator leaves a tool
    /// out of the specs it advertises when this is `false` (e.g. an agent with no reachable SSH
    /// host), so it never sees a tool it can't use.
    fn is_available(&self) -> bool {
        true
    }
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
