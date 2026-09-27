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

/// The five node tools for this hub. `audit_path`: where every call is logged (`None`: not logged).
pub fn node_tools(registry: NodeRegistry, config_path: PathBuf, devices_path: PathBuf, audit_path: Option<PathBuf>) -> Vec<Arc<dyn Tool>> {
    let ctx = NodeContext { registry, config_path, devices_path, agent: None, approver: None, audit: audit_path.map(|p| Arc::new(AuditLog::new(p))) };
    [NodeOp::List, NodeOp::Shell, NodeOp::ReadFile, NodeOp::WriteFile, NodeOp::ListFiles]
        .into_iter()
        .map(|op| Arc::new(NodeTool { ctx: ctx.clone(), op }) as Arc<dyn Tool>)
        .collect()
}

fn str_arg<'a>(args: &'a Value, name: &str) -> anyhow::Result<&'a str> {
    args.get(name).and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required '{name}' argument"))
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
                json!({
                    "node": u.id,
                    "name": u.node.name,
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
        // The file content isn't worth keeping in the log; what was done, where and by whom is.
        let mut logged = args.clone();
        if let Some(content) = logged.get_mut("content") {
            *content = json!(format!("({} characters)", content.as_str().map_or(0, |c| c.chars().count())));
        }
        self.ctx.log(&node, &name, &logged, &result);
        result
    }
}
