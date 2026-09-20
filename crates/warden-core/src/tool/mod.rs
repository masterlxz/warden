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

    /// A copy of this tool that only reaches the tools named in `allowed`, or `None` when it has no
    /// tools of its own to restrict. Called by `Orchestrator::with_allowed_tools` on every tool that
    /// survives the filter: a tool that runs a nested agent (`delegate_task`) overrides it so the
    /// sub-agent can't reach what its caller may not.
    fn restricted_to(&self, _allowed: &[String]) -> Option<Arc<dyn Tool>> {
        None
    }

    /// Whether the model should be offered this tool right now. The orchestrator leaves a tool
    /// out of the specs it advertises when this is `false` (e.g. an agent with no reachable SSH
    /// host), so it never sees a tool it can't use.
    fn is_available(&self) -> bool {
        true
    }

    /// A copy of this tool that asks `approver` before doing anything the user configured as
    /// needing a human "yes" (`ssh_*` on a host with `require_approval`), or `None` when the tool
    /// never asks. Called by `Orchestrator::with_approver`; a channel that can't ask never calls
    /// it, so those tools refuse instead of running unattended.
    fn with_approver(&self, _approver: Arc<dyn Approver>) -> Option<Arc<dyn Tool>> {
        None
    }
}

/// What a tool wants a human to confirm: what it acts on (an SSH server id, an agent id), what kind
/// of action (`exec`, `upload`, `download`, `create_agent`, `update_agent`), and the exact command
/// line, file paths or text that will be applied.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    pub target: String,
    pub action: String,
    pub detail: String,
}

/// Something that can put an `ApprovalRequest` in front of the user and wait for the answer — a
/// modal in the desktop, a `[y/N]` card in the CLI. Anything that can't answer (no reply within
/// the tool's deadline included) counts as "no".
#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, request: ApprovalRequest) -> bool;
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
