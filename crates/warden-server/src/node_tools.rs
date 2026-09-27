//! The tools agents use on nodes (P93): `list_nodes`, `node_shell`, `node_read_file`,
//! `node_write_file` and `node_list_files`. Generic tools with a `node` parameter (the user's choice):
//! the tool list doesn't grow with every node that joins.
//!
//! A node is usable by a turn only when all of these hold, checked again on every call so a change
//! applies without a restart: it's connected, approved in the device list, switched on in `[[nodes]]`,
//! open to this agent (an empty list means every agent), and offers what the call needs. With
//! `require_approval`, every call waits for a person's yes; a channel that can't ask refuses. Built
//! like the SSH tools (`warden_core::tool::ssh`): scoped to the agent per turn, given the approver per
//! turn, hidden when there's nothing to use, and every call logged.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use warden_bootstrap::{load_config_from_path, NodeAccessConfig};
use warden_core::tool::ssh::AuditLog;
use warden_core::tool::{ApprovalRequest, Approver, Tool, ToolSpec};

use crate::device_registry::{PairingStatus, PairingStore};
use crate::nodes::{ConnectedNode, NodeRegistry};

const DEFAULT_SHELL_TIMEOUT_MS: u64 = 30_000;
const MAX_SHELL_TIMEOUT_MS: u64 = 300_000;
/// On top of the command's own timeout: the node answers with a timeout error itself, and this only
/// covers the trip back.
const TIMEOUT_MARGIN: Duration = Duration::from_secs(10);
const FILE_TIMEOUT: Duration = Duration::from_secs(30);
/// MCP tools do anything from a lookup to a long job; the node's own server decides beyond this.
const MCP_TIMEOUT: Duration = Duration::from_secs(300);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const DETAIL_PREVIEW_CHARS: usize = 2000;

/// What every node tool shares for one turn.
#[derive(Clone)]
pub struct NodeContext {
    registry: NodeRegistry,
    config_path: PathBuf,
    /// The device registry's file: a node must be approved there.
    devices_path: PathBuf,
    agent: Option<String>,
    approver: Option<Arc<dyn Approver>>,
    audit: Option<Arc<AuditLog>>,
}

/// A node this turn may use, with what it offers and the hub's rules for it.
struct Usable {
    id: String,
    node: ConnectedNode,
    access: NodeAccessConfig,
}

impl NodeContext {
    fn usable(&self) -> Vec<Usable> {
        let online = self.registry.online();
        if online.is_empty() {
            return Vec::new();
        }
        let Ok(config) = load_config_from_path(&self.config_path, false) else { return Vec::new() };
        online
            .into_iter()
            .filter_map(|(id, node)| {
                let access = config.nodes.iter().find(|n| n.id == id && n.enabled)?.clone();
                let open = access.agents.is_empty() || self.agent.as_ref().is_some_and(|a| access.agents.contains(a));
                let approved = matches!(PairingStore::new(self.devices_path.clone()).status(&id), Ok(Some(PairingStatus::Approved)));
                (open && approved).then_some(Usable { id, node, access })
            })
            .collect()
    }

    fn pick(&self, id: &str, need: Need) -> anyhow::Result<Usable> {
        let usable = self.usable();
        let names: Vec<String> = usable.iter().filter(|u| need.met_by(&u.node)).map(|u| u.id.clone()).collect();
        let Some(found) = usable.into_iter().find(|u| u.id == id) else {
            anyhow::bail!("no node '{id}' you can use right now (available: {}) — see list_nodes", if names.is_empty() { "none".to_string() } else { names.join(", ") });
        };
        anyhow::ensure!(need.met_by(&found.node), "node '{id}' doesn't offer {}", need.label());
        Ok(found)
    }

    async fn approve(&self, node: &Usable, action: &str, detail: String) -> anyhow::Result<()> {
        if !node.access.require_approval {
            return Ok(());
        }
        let Some(approver) = &self.approver else {
            anyhow::bail!("node '{}' needs a person's approval for every call, and nobody can be asked here — nothing was run", node.id);
        };
        let request = ApprovalRequest { target: node.id.clone(), action: action.to_string(), detail };
        if !tokio::time::timeout(APPROVAL_TIMEOUT, approver.approve(request)).await.unwrap_or(false) {
            anyhow::bail!("the user did not approve this on node '{}'", node.id);
        }
        Ok(())
    }

    fn log(&self, node: &str, op: &str, args: &Value, result: &anyhow::Result<Value>) {
        let Some(audit) = &self.audit else { return };
        let entry = json!({
            "ts_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            "node": node,
            "agent": self.agent,
            "op": op,
            "args": args,
            "ok": result.is_ok(),
            "error": result.as_ref().err().map(|e| format!("{e:#}")),
        });
        if let Err(err) = audit.record(&entry) {
            eprintln!("warden-server: can't write the node audit log: {err}");
        }
    }
}

#[derive(Clone, Copy)]
enum Need {
    Shell,
    Files,
}

impl Need {
    fn met_by(self, node: &ConnectedNode) -> bool {
        match self {
            Need::Shell => node.offer.shell,
            Need::Files => node.offer.files,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Need::Shell => "a shell",
            Need::Files => "files",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NodeOp {
    List,
    Shell,
    ReadFile,
    WriteFile,
    ListFiles,
}

pub struct NodeTool {
    ctx: NodeContext,
    op: NodeOp,
}

/// Builds this hub's node tools: the five fixed ones, and one per MCP tool a connected node lends.
#[derive(Clone)]
pub struct NodeToolFactory {
    ctx: NodeContext,
}

/// The longest tool name the model APIs accept.
const MAX_TOOL_NAME: usize = 64;

impl NodeToolFactory {
    /// `audit_path`: where every call is logged (`None`: not logged).
    pub fn new(registry: NodeRegistry, config_path: PathBuf, devices_path: PathBuf, audit_path: Option<PathBuf>) -> Self {
        Self { ctx: NodeContext { registry, config_path, devices_path, agent: None, approver: None, audit: audit_path.map(|p| Arc::new(AuditLog::new(p))) } }
    }

    /// `list_nodes`, `node_shell`, `node_read_file`, `node_write_file`, `node_list_files`.
    pub fn fixed_tools(&self) -> Vec<Arc<dyn Tool>> {
        [NodeOp::List, NodeOp::Shell, NodeOp::ReadFile, NodeOp::WriteFile, NodeOp::ListFiles]
            .into_iter()
            .map(|op| Arc::new(NodeTool { ctx: self.ctx.clone(), op }) as Arc<dyn Tool>)
            .collect()
    }

    /// One tool per MCP tool of every connected node, named `<node>__<tool>` with its own schema.
    /// Rebuilt whenever a node joins or leaves; who may use each is still checked per turn.
    pub fn mcp_tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut taken: Vec<String> = Vec::new();
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for (id, node) in self.ctx.registry.online() {
            let slug = node_slug(&node.name);
            for spec in &node.offer.mcp_tools {
                let name = mcp_tool_name(&slug, &id, &spec.name, &taken);
                taken.push(name.clone());
                let mut offered = spec.clone();
                offered.name = name;
                offered.description = format!("On node '{}': {}", node.name, spec.description);
                tools.push(Arc::new(NodeMcpTool { ctx: self.ctx.clone(), node_id: id.clone(), remote_name: spec.name.clone(), spec: offered }));
            }
        }
        tools
    }
}

/// `"Casa PC"` → `"casa-pc"`: what the model sees before `__` in a node's MCP tools.
pub fn node_slug(name: &str) -> String {
    let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let slug = slug.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let slug: String = slug.chars().take(20).collect();
    if slug.is_empty() { "node".to_string() } else { slug }
}

/// `<slug>__<tool>`, unique among `taken` (two nodes with the same name get a piece of the id) and at
/// most 64 characters.
fn mcp_tool_name(slug: &str, node_id: &str, tool: &str, taken: &[String]) -> String {
    let fit = |prefix: &str| {
        let room = MAX_TOOL_NAME.saturating_sub(prefix.len() + 2);
        format!("{prefix}__{}", tool.chars().take(room).collect::<String>())
    };
    let name = fit(slug);
    if !taken.contains(&name) {
        return name;
    }
    let tail: String = node_id.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().filter(|c| c.is_ascii_alphanumeric()).collect();
    fit(&format!("{slug}-{tail}"))
}

fn str_arg<'a>(args: &'a Value, name: &str) -> anyhow::Result<&'a str> {
    args.get(name).and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required '{name}' argument"))
}

/// Longer strings than this go in the log as their size only.
const LOG_TEXT_CHARS: usize = 200;

/// The arguments as the audit log keeps them: what was done, where and by whom — not the file
/// content, nor any other long text an MCP tool was handed.
fn for_log(args: &Value) -> Value {
    let Some(fields) = args.as_object() else { return args.clone() };
    let size = |text: &str| json!(format!("({} characters)", text.chars().count()));
    Value::Object(
        fields
            .iter()
            .map(|(key, value)| {
                let kept = match value.as_str() {
                    Some(text) if key == "content" || text.chars().count() > LOG_TEXT_CHARS => size(text),
                    _ => value.clone(),
                };
                (key.clone(), kept)
            })
            .collect(),
    )
}

fn preview(text: &str) -> String {
    if text.chars().count() > DETAIL_PREVIEW_CHARS {
        format!("{}… ({} characters in all)", text.chars().take(DETAIL_PREVIEW_CHARS).collect::<String>(), text.chars().count())
    } else {
        text.to_string()
    }
}

impl NodeTool {
    fn node_param(&self, need: Need) -> Value {
        let ids: Vec<String> = self.ctx.usable().into_iter().filter(|u| need.met_by(&u.node)).map(|u| u.id).collect();
        let mut param = json!({ "type": "string", "description": "Which node, by id (see list_nodes)." });
        if !ids.is_empty() {
            param["enum"] = json!(ids);
        }
        param
    }

    fn list(&self) -> Value {
        let nodes: Vec<Value> = self
            .ctx
            .usable()
            .into_iter()
            .map(|u| {
                let mut offers = Vec::new();
                if u.node.offer.shell {
                    offers.push("shell");
                }
                if u.node.offer.files {
                    offers.push("files");
                }
                let mcp: Vec<&str> = u.node.offer.mcp_tools.iter().map(|t| t.name.as_str()).collect();
                if !mcp.is_empty() {
                    offers.push("mcp");
                }
                json!({
                    "node": u.id,
                    "name": u.node.name,
                    "mcp_tools": mcp,
                    "mcp_tool_prefix": format!("{}__", node_slug(&u.node.name)),
                    "description": u.node.offer.description,
                    "tags": u.node.offer.tags,
                    "offers": offers,
                    "asks_before_each_call": u.access.require_approval,
                })
            })
            .collect();
        json!({ "nodes": nodes })
    }

    /// Sends `local_tool(args)` to the node and waits. A node that drops mid-call fails it at once,
    /// and it is never retried: a shell command isn't safe to run twice.
    async fn run_on(&self, node: &Usable, local_tool: &str, args: Value, timeout: Duration) -> anyhow::Result<Value> {
        node.node.channel.call(local_tool.to_string(), args, timeout).await.map_err(|err| {
            let text = format!("{err:#}");
            if text.contains("connection closed") {
                anyhow::anyhow!("node '{}' disconnected in the middle of the call; it was not retried", node.id)
            } else {
                anyhow::anyhow!("node '{}': {text}", node.id)
            }
        })
    }

    async fn call_op(&self, args: &Value) -> anyhow::Result<(String, Value)> {
        let id = str_arg(args, "node")?.to_string();
        let result = match self.op {
            NodeOp::List => unreachable!("list_nodes has no node"),
            NodeOp::Shell => {
                let node = self.ctx.pick(&id, Need::Shell)?;
                let command = str_arg(args, "command")?;
                let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_SHELL_TIMEOUT_MS).min(MAX_SHELL_TIMEOUT_MS);
                let cwd = args.get("cwd").and_then(Value::as_str);
                let detail = format!("Run on node '{}' ({}){}:\n\n{command}", node.id, node.node.name, cwd.map(|c| format!(" in {c}")).unwrap_or_default());
                self.ctx.approve(&node, "node_shell", detail).await?;
                let mut local = json!({ "command": command, "timeout_ms": timeout_ms });
                if let Some(cwd) = cwd {
                    local["cwd"] = json!(cwd);
                }
                self.run_on(&node, "shell", local, Duration::from_millis(timeout_ms) + TIMEOUT_MARGIN).await?
            }
            NodeOp::ReadFile => {
                let node = self.ctx.pick(&id, Need::Files)?;
                let path = str_arg(args, "path")?;
                self.ctx.approve(&node, "node_read_file", format!("Read '{path}' on node '{}' ({})", node.id, node.node.name)).await?;
                self.run_on(&node, "read_file", json!({ "path": path }), FILE_TIMEOUT).await?
            }
            NodeOp::WriteFile => {
                let node = self.ctx.pick(&id, Need::Files)?;
                let path = str_arg(args, "path")?;
                let content = str_arg(args, "content")?;
                let detail = format!("Write '{path}' on node '{}' ({}):\n\n{}", node.id, node.node.name, preview(content));
                self.ctx.approve(&node, "node_write_file", detail).await?;
                self.run_on(&node, "write_file", json!({ "path": path, "content": content }), FILE_TIMEOUT).await?
            }
            NodeOp::ListFiles => {
                let node = self.ctx.pick(&id, Need::Files)?;
                let path = args.get("path").and_then(Value::as_str).unwrap_or("");
                self.ctx.approve(&node, "node_list_files", format!("List '{path}' on node '{}' ({})", node.id, node.node.name)).await?;
                self.run_on(&node, "list_files", json!({ "path": path }), FILE_TIMEOUT).await?
            }
        };
        Ok((id, result))
    }
}

#[async_trait]
impl Tool for NodeTool {
    fn spec(&self) -> ToolSpec {
        let (name, description, parameters) = match self.op {
            NodeOp::List => (
                "list_nodes",
                "List the other machines (nodes) you can use: their id, what they are, tags like 'gpu' or 'home', \
                 and what each offers ('shell', 'files'). Use node_shell/node_read_file/node_write_file/node_list_files \
                 with a node's id to work on it.",
                json!({ "type": "object", "properties": {} }),
            ),
            NodeOp::Shell => (
                "node_shell",
                "Run a shell command on another machine (a node) and get its exit code, stdout and stderr. Nothing \
                 is retried: if the node drops mid-command, you get an error and it's up to you what to do.",
                json!({
                    "type": "object",
                    "properties": {
                        "node": self.node_param(Need::Shell),
                        "command": { "type": "string", "description": "The command line to run on the node." },
                        "cwd": { "type": "string", "description": "Working directory on the node. Defaults to the node's own." },
                        "timeout_ms": { "type": "number", "description": "Max time for the command, in milliseconds. Defaults to 30000, capped at 300000." }
                    },
                    "required": ["node", "command"]
                }),
            ),
            NodeOp::ReadFile => (
                "node_read_file",
                "Read a text file from the folder a node shares, by its path relative to that folder.",
                json!({
                    "type": "object",
                    "properties": {
                        "node": self.node_param(Need::Files),
                        "path": { "type": "string", "description": "Path relative to the node's shared folder." }
                    },
                    "required": ["node", "path"]
                }),
            ),
            NodeOp::WriteFile => (
                "node_write_file",
                "Create or overwrite a text file in the folder a node shares.",
                json!({
                    "type": "object",
                    "properties": {
                        "node": self.node_param(Need::Files),
                        "path": { "type": "string", "description": "Path relative to the node's shared folder." },
                        "content": { "type": "string", "description": "The whole file content." }
                    },
                    "required": ["node", "path", "content"]
                }),
            ),
            NodeOp::ListFiles => (
                "node_list_files",
                "List the files in the folder a node shares (or in a subfolder of it).",
                json!({
                    "type": "object",
                    "properties": {
                        "node": self.node_param(Need::Files),
                        "path": { "type": "string", "description": "Subfolder, relative to the shared folder. Empty for all of it." }
                    },
                    "required": ["node"]
                }),
            ),
        };
        ToolSpec { name: name.to_string(), description: description.to_string(), parameters }
    }

    fn scoped_to_agent(&self, agent: Option<&str>) -> Option<Arc<dyn Tool>> {
        let mut ctx = self.ctx.clone();
        ctx.agent = agent.map(str::to_string);
        Some(Arc::new(Self { ctx, op: self.op }))
    }

    fn with_approver(&self, approver: Arc<dyn Approver>) -> Option<Arc<dyn Tool>> {
        let mut ctx = self.ctx.clone();
        ctx.approver = Some(approver);
        Some(Arc::new(Self { ctx, op: self.op }))
    }

    fn is_available(&self) -> bool {
        let usable = self.ctx.usable();
        match self.op {
            NodeOp::List => !usable.is_empty(),
            NodeOp::Shell => usable.iter().any(|u| Need::Shell.met_by(&u.node)),
            NodeOp::ReadFile | NodeOp::WriteFile | NodeOp::ListFiles => usable.iter().any(|u| Need::Files.met_by(&u.node)),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        if self.op == NodeOp::List {
            return Ok(self.list());
        }
        let name = self.spec().name;
        let node = args.get("node").and_then(Value::as_str).unwrap_or("").to_string();
        let result = self.call_op(&args).await.map(|(_, value)| value);
        self.ctx.log(&node, &name, &for_log(&args), &result);
        result
    }
}

/// One MCP tool a node lends, offered to the model under its own name and schema (P93, fatia 2). The
/// same rules as the other node tools apply on every call: the node online, approved, switched on and
/// open to this agent; a yes first when it asks; logged; never retried if the node drops.
pub struct NodeMcpTool {
    ctx: NodeContext,
    node_id: String,
    /// The name on the node.
    remote_name: String,
    spec: ToolSpec,
}

impl NodeMcpTool {
    fn usable(&self) -> Option<Usable> {
        self.ctx.usable().into_iter().find(|u| u.id == self.node_id && u.node.offer.mcp_tools.iter().any(|t| t.name == self.remote_name))
    }
}

#[async_trait]
impl Tool for NodeMcpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn scoped_to_agent(&self, agent: Option<&str>) -> Option<Arc<dyn Tool>> {
        let mut ctx = self.ctx.clone();
        ctx.agent = agent.map(str::to_string);
        Some(Arc::new(Self { ctx, node_id: self.node_id.clone(), remote_name: self.remote_name.clone(), spec: self.spec.clone() }))
    }

    fn with_approver(&self, approver: Arc<dyn Approver>) -> Option<Arc<dyn Tool>> {
        let mut ctx = self.ctx.clone();
        ctx.approver = Some(approver);
        Some(Arc::new(Self { ctx, node_id: self.node_id.clone(), remote_name: self.remote_name.clone(), spec: self.spec.clone() }))
    }

    fn is_available(&self) -> bool {
        self.usable().is_some()
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let result = async {
            let node = self.usable().ok_or_else(|| anyhow::anyhow!("node '{}' isn't available to you right now", self.node_id))?;
            let detail = format!(
                "Call '{}' on node '{}' ({}) with:\n\n{}",
                self.remote_name,
                node.id,
                node.node.name,
                preview(&serde_json::to_string_pretty(&args).unwrap_or_default())
            );
            self.ctx.approve(&node, &self.spec.name, detail).await?;
            let tool = NodeTool { ctx: self.ctx.clone(), op: NodeOp::List };
            tool.run_on(&node, "mcp", json!({ "tool": self.remote_name, "arguments": args }), MCP_TIMEOUT).await
        }
        .await;
        self.ctx.log(&self.node_id, &self.spec.name, &for_log(&args), &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_what_was_done_not_the_text() {
        let logged = for_log(&json!({ "path": "a.md", "content": "secret", "query": "x".repeat(300), "limit": 5 }));
        assert_eq!(logged, json!({ "path": "a.md", "content": "(6 characters)", "query": "(300 characters)", "limit": 5 }));
    }

    #[test]
    fn mcp_tool_names_are_short_safe_and_unique() {
        assert_eq!(node_slug("Casa PC!"), "casa-pc");
        assert_eq!(node_slug("  "), "node");
        assert_eq!(mcp_tool_name("casa-pc", "node-casa-pc-1a2b3c4d", "query", &[]), "casa-pc__query");
        let taken = vec!["casa-pc__query".to_string()];
        assert_eq!(mcp_tool_name("casa-pc", "node-casa-pc-1a2b3c4d", "query", &taken), "casa-pc-1a2b3c4d__query");
        let long = mcp_tool_name("casa-pc", "id", &"x".repeat(100), &[]);
        assert_eq!(long.len(), MAX_TOOL_NAME);
        assert!(long.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }
}
