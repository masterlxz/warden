use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::budget::TurnBudget;
use crate::jobs::JobBoard;

pub mod delegate;
pub mod delegate_to_agent;
pub mod document;
pub mod file_tools;
pub mod job_tools;
pub mod mcp;
pub mod mcp_oauth;
pub mod shell;
pub mod skill_tools;
pub mod spend_tool;
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

    /// A copy of this tool whose nested orchestrators (sub-agents) are charged to `budget`, or
    /// `None` when it runs none. Called by `Orchestrator::with_turn_budget`/`charged_to` at the start
    /// of a turn so the whole tree of sub-agents spends from one `TurnBudget`. A tool that only needs
    /// to read the turn's spending limits (`budget`) binds to it the same way.
    fn with_budget(&self, _budget: &Arc<TurnBudget>) -> Option<Arc<dyn Tool>> {
        None
    }

    /// A copy of this tool bound to `board`, the background jobs of the turn that is starting, or
    /// `None` when it has nothing to do with jobs. Called by `Orchestrator::with_turn_jobs` on the
    /// turn's root only: the delegation tools use it to accept `background: true`, and the `jobs` tool
    /// to read the results. Until a tool is bound it should keep itself out of the model's sight
    /// (`is_available`/its spec), so nothing advertises a feature that can't work.
    fn with_jobs(&self, _board: &Arc<JobBoard>) -> Option<Arc<dyn Tool>> {
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

/// The name a tool should register under, given the names already claimed: `tool_name` unchanged
/// if nothing else has it yet, otherwise `"{namespace}__{tool_name}"`. Two unrelated call sites
/// hit this — `warden-bootstrap::register_mcp_tools` (two MCP servers, or an MCP server and a
/// built-in tool, advertising the same name; `namespace` is the server's config name) and
/// `warden-server`'s per-connection tool registration (a remote client's advertised tool colliding
/// with the shared `Orchestrator`'s own tools, P42; `namespace` is the client's `device_id`) — kept
/// here, not duplicated in each crate, since it's the same rule either way. `__` rather than `.`/
/// `-` because every function-calling API this project talks to (OpenAI/Gemini/Anthropic)
/// restricts tool names to `[a-zA-Z0-9_-]`. Pure and rename-only-when-needed on purpose: a tool
/// nobody collides with keeps the exact name a person may already have written into
/// `allowed_tools`, a skill or a habit.
pub fn dedupe_tool_name(existing: &[String], namespace: &str, tool_name: &str) -> String {
    if existing.iter().any(|n| n == tool_name) {
        format!("{namespace}__{tool_name}")
    } else {
        tool_name.to_string()
    }
}

/// A tool with its `spec().name` overridden — the mechanism behind `rename_tool`, used only to
/// disambiguate a tool (an MCP server's, or a remote client's) whose bare name collides with one
/// already registered (P46/P42). Every other trait method delegates to `inner`, re-wrapping
/// whatever a "copy of this tool, but..." method returns so the rename survives
/// `with_allowed_tools`/`with_budget`/etc. — without that, the copy would silently revert to the
/// original, unprefixed name.
struct NamespacedTool {
    inner: Arc<dyn Tool>,
    name: String,
}

/// Wraps `tool` so `spec().name` reads as `name` instead of whatever `tool` itself reports,
/// leaving everything else (behavior, parameters, description, and — crucially — what `call()`
/// actually sends downstream) untouched. `McpTool::call`/`RemoteTool::call` both build their
/// outgoing request from their own internal `spec`, never from what this wrapper reports, so
/// renaming here never desyncs from what an MCP server or a connected client itself expects to be
/// called. A tool is only ever wrapped this way when its bare name would otherwise be
/// unreachable, never as a matter of course.
pub fn rename_tool(tool: Arc<dyn Tool>, name: impl Into<String>) -> Arc<dyn Tool> {
    Arc::new(NamespacedTool { inner: tool, name: name.into() })
}

#[async_trait]
impl Tool for NamespacedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: self.name.clone(), ..self.inner.spec() }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        self.inner.call(args).await
    }

    fn scoped_to_agent(&self, agent: Option<&str>) -> Option<Arc<dyn Tool>> {
        self.inner.scoped_to_agent(agent).map(|t| rename_tool(t, self.name.clone()))
    }

    fn restricted_to(&self, allowed: &[String]) -> Option<Arc<dyn Tool>> {
        self.inner.restricted_to(allowed).map(|t| rename_tool(t, self.name.clone()))
    }

    fn with_budget(&self, budget: &Arc<TurnBudget>) -> Option<Arc<dyn Tool>> {
        self.inner.with_budget(budget).map(|t| rename_tool(t, self.name.clone()))
    }

    fn with_jobs(&self, board: &Arc<JobBoard>) -> Option<Arc<dyn Tool>> {
        self.inner.with_jobs(board).map(|t| rename_tool(t, self.name.clone()))
    }

    fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    fn with_approver(&self, approver: Arc<dyn Approver>) -> Option<Arc<dyn Tool>> {
        self.inner.with_approver(approver).map(|t| rename_tool(t, self.name.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_tool_name_only_renames_on_a_real_collision() {
        assert_eq!(dedupe_tool_name(&[], "anchor", "search"), "search");
        assert_eq!(dedupe_tool_name(&["read_file".to_string(), "shell".to_string()], "anchor", "search"), "search");
        assert_eq!(dedupe_tool_name(&["search".to_string()], "anchor", "search"), "anchor__search");
        // Colliding with a name another namespace already claimed (an MCP server, a remote
        // client's own advertised name) works the same way as colliding with a built-in.
        assert_eq!(dedupe_tool_name(&["docs__search".to_string(), "search".to_string()], "anchor", "search"), "anchor__search");
    }

    /// A minimal `Tool` whose "copy of this tool, but..." methods actually produce a new copy
    /// (unlike the trait's `None` defaults) — needed to prove `NamespacedTool` re-wraps them
    /// instead of losing the rename on the first such call.
    struct Probe {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for Probe {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: self.name.to_string(), description: "probe".to_string(), parameters: Value::Null }
        }

        async fn call(&self, args: Value) -> anyhow::Result<Value> {
            Ok(args)
        }

        fn with_budget(&self, _budget: &Arc<TurnBudget>) -> Option<Arc<dyn Tool>> {
            Some(Arc::new(Probe { name: self.name }))
        }
    }

    #[tokio::test]
    async fn renamed_tool_reports_the_new_name_but_keeps_calling_through() {
        let probe = Arc::new(Probe { name: "search" });
        let renamed = rename_tool(probe, "anchor__search");

        assert_eq!(renamed.spec().name, "anchor__search");
        assert_eq!(renamed.call(serde_json::json!({"q": 1})).await.unwrap(), serde_json::json!({"q": 1}));
    }

    #[tokio::test]
    async fn a_copy_produced_through_with_budget_keeps_the_rename() {
        let probe = Arc::new(Probe { name: "search" });
        let renamed = rename_tool(probe, "anchor__search");

        let budget = TurnBudget::for_turn(None, None);
        let copy = renamed.with_budget(&budget).expect("Probe always produces a copy");

        // The name survived the round trip through the inner tool's own with_budget, not just
        // the first wrap — this is the behavior a rename would silently lose without re-wrapping.
        assert_eq!(copy.spec().name, "anchor__search");
    }

    #[test]
    fn a_tool_with_no_copy_methods_leaves_them_none_through_the_wrapper() {
        struct Bare;
        #[async_trait]
        impl Tool for Bare {
            fn spec(&self) -> ToolSpec {
                ToolSpec { name: "bare".to_string(), description: String::new(), parameters: Value::Null }
            }
            async fn call(&self, _args: Value) -> anyhow::Result<Value> {
                Ok(Value::Null)
            }
        }
        let renamed = rename_tool(Arc::new(Bare), "srv__bare");
        assert!(renamed.scoped_to_agent(None).is_none());
        assert!(renamed.restricted_to(&[]).is_none());
        assert!(renamed.with_jobs(&JobBoard::new(1)).is_none());
        assert_eq!(renamed.spec().name, "srv__bare");
    }
}
