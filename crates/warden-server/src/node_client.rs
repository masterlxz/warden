//! The node side of P93: `warden-server node --hub wss://…` connects to a hub as one more device,
//! announces what this machine lends (a shell, a folder of files) and runs the calls the hub's agents
//! make on it. Nothing else lives here: no vault, no agents, no model — the state stays on the hub.
//!
//! This is the node's own lock of the two: whatever wasn't switched on here (`--shell`, `--files`) is
//! refused here too, whatever the hub asks. Files never leave the chosen folder (`Vault::path_of`
//! refuses absolute paths and `..`).
//!
//! The desktop runs the same thing (P97, "lend this computer"): it adds an activity log and watches
//! the connection's state, which the CLI prints instead.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};
use std::collections::HashMap;

use futures_util::StreamExt;
use warden_core::memory::Vault;
use warden_core::model::fallback::is_transient;
use warden_core::model::{Message, ModelProvider, StreamEvent};
use warden_core::tool::ToolSpec;
use warden_core::tool::shell::ShellTool;
use warden_core::tool::Tool;
use warden_server_protocol::protocol::NodeOfferDto;
use warden_server_protocol::{AuthRejected, ClientMessage, ServerConnection, ServerMessage};

const HEARTBEAT: Duration = Duration::from_secs(20);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Biggest file `read_file` sends back: this is text for a model, not a transfer tool.
const MAX_READ_BYTES: u64 = 1024 * 1024;
const MAX_LISTED_FILES: usize = 1000;
/// How many calls the activity log keeps — the newest ones.
pub const MAX_ACTIVITY: usize = 200;

/// One call the hub's agents made on this machine.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    /// Seconds since the Unix epoch.
    pub at: u64,
    /// `shell`, `read_file`, `write_file`, `list_files`, `mcp` or `model`.
    pub kind: String,
    /// The command, the path, the MCP tool or the model id.
    pub summary: String,
    pub error: Option<String>,
}

/// The last `MAX_ACTIVITY` calls, shared between the node and whoever shows them (the desktop).
#[derive(Clone, Default)]
pub struct NodeActivity(Arc<Mutex<VecDeque<ActivityEntry>>>);

impl NodeActivity {
    fn record(&self, kind: &str, summary: String, error: Option<String>) {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut log = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if log.len() == MAX_ACTIVITY {
            log.pop_front();
        }
        log.push_back(ActivityEntry { at, kind: kind.to_string(), summary, error });
    }

    /// Newest first.
    pub fn entries(&self) -> Vec<ActivityEntry> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).iter().rev().cloned().collect()
    }
}

/// Where the connection to the hub stands, for whoever shows it (the desktop).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum NodeState {
    Connecting,
    Connected,
    /// The last try failed; the next one is `in_secs` away.
    #[serde(rename_all = "camelCase")]
    Retrying { error: String, in_secs: u64 },
    /// Gave up: retrying can't fix this (not paired, or the hub turned it away).
    Stopped { error: String },
}

/// What this machine runs for the hub.
pub struct LocalNode {
    /// Where `shell` runs by default, when the shell is on.
    shell: Option<ShellTool>,
    /// The shared folder, when files are on.
    files: Option<Arc<Vault>>,
    /// Tools of the MCP servers lent with `--mcp` (fatia 2), already named uniquely.
    mcp: Vec<Arc<dyn Tool>>,
    /// Model providers lent with `--model` (fatia 3), by their id in this machine's config.
    models: Vec<(String, Arc<dyn ModelProvider>)>,
    /// Where each call is written down, when someone watches (the desktop).
    activity: Option<NodeActivity>,
}

impl LocalNode {
    /// `shell`: run commands (in `files`' folder, or the home directory). `files`: the folder shared.
    pub fn new(shell: bool, files: Option<PathBuf>) -> Self {
        let base = files.clone().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
        Self { shell: shell.then(|| ShellTool::new(Arc::new(Vault::new(base)))), files: files.map(|dir| Arc::new(Vault::new(dir))), mcp: Vec::new(), models: Vec::new(), activity: None }
    }

    /// Writes every call the hub makes here to `activity`.
    pub fn with_activity(mut self, activity: NodeActivity) -> Self {
        self.activity = Some(activity);
        self
    }

    /// Also lends these MCP tools (from `warden_bootstrap::connect_mcp_server`, names already unique).
    pub fn with_mcp_tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.mcp = tools;
        self
    }

    /// Also lends these model providers (`warden_bootstrap::build_model_for`), by their id here.
    pub fn with_models(mut self, models: Vec<(String, Arc<dyn ModelProvider>)>) -> Self {
        self.models = models;
        self
    }

    pub fn offer(&self, description: String, tags: Vec<String>) -> NodeOfferDto {
        NodeOfferDto {
            description,
            tags,
            shell: self.shell.is_some(),
            files: self.files.is_some(),
            mcp_tools: self.mcp.iter().map(|t| t.spec()).collect(),
            models: self.models.iter().map(|(id, _)| id.clone()).collect(),
        }
    }

    /// Runs one model call for the hub, sending each event on `reply` and ending with `ModelDone` or
    /// `ModelError`. Fallback notices from a combo on this node stay here: the hub reports its own.
    /// An answer the hub cancels mid-way (`ModelCancel` aborts the task) doesn't reach the log.
    pub async fn answer_model(&self, request_id: u64, model: &str, messages: Vec<Message>, tools: Vec<ToolSpec>, reply: &mpsc::UnboundedSender<ClientMessage>) {
        let error = self.answer_model_inner(request_id, model, messages, tools, reply).await;
        if let Some(activity) = &self.activity {
            activity.record("model", model.to_string(), error);
        }
    }

    /// The error it sent the hub, if any.
    async fn answer_model_inner(&self, request_id: u64, model: &str, messages: Vec<Message>, tools: Vec<ToolSpec>, reply: &mpsc::UnboundedSender<ClientMessage>) -> Option<String> {
        let Some((_, provider)) = self.models.iter().find(|(id, _)| id == model) else {
            let message = format!("this node doesn't lend a model '{model}'");
            let _ = reply.send(ClientMessage::ModelError { request_id, message: message.clone(), transient: false });
            return Some(message);
        };
        let mut stream = match provider.chat_stream(messages, tools).await {
            Ok(stream) => stream,
            Err(err) => {
                let message = format!("{err:#}");
                let _ = reply.send(ClientMessage::ModelError { request_id, message: message.clone(), transient: is_transient(&err) });
                return Some(message);
            }
        };
        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamEvent::ProviderFallback(_)) => {}
                Ok(event) => {
                    if reply.send(ClientMessage::ModelEvent { request_id, event }).is_err() {
                        return Some("the connection to the hub closed mid-answer".to_string());
                    }
                }
                Err(err) => {
                    let message = format!("{err:#}");
                    let _ = reply.send(ClientMessage::ModelError { request_id, message: message.clone(), transient: false });
                    return Some(message);
                }
            }
        }
        let _ = reply.send(ClientMessage::ModelDone { request_id });
        None
    }

    /// Runs one call from the hub: `shell`, `read_file`, `write_file`, `list_files` or `mcp`.
    pub async fn run(&self, tool: &str, args: Value) -> anyhow::Result<Value> {
        let summary = activity_summary(tool, &args);
        let result = self.run_inner(tool, args).await;
        if let Some(activity) = &self.activity {
            activity.record(tool, summary, result.as_ref().err().map(|err| format!("{err:#}")));
        }
        result
    }

    async fn run_inner(&self, tool: &str, args: Value) -> anyhow::Result<Value> {
        match tool {
            "shell" => {
                let shell = self.shell.as_ref().ok_or_else(|| anyhow::anyhow!("this node doesn't lend its shell (start it with --shell)"))?;
                shell.call(args).await
            }
            "read_file" | "write_file" | "list_files" => {
                let vault = self.files.as_ref().ok_or_else(|| anyhow::anyhow!("this node doesn't share files (start it with --files <folder>)"))?;
                let path = args.get("path").and_then(Value::as_str).unwrap_or("");
                match tool {
                    "read_file" => read_file(vault, path),
                    "write_file" => {
                        let content = args.get("content").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'content' argument"))?;
                        vault.write(path, content)?;
                        Ok(json!({ "status": "ok", "path": path }))
                    }
                    _ => list_files(vault, path),
                }
            }
            "mcp" => {
                let name = args.get("tool").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'tool' argument"))?;
                let tool = self.mcp.iter().find(|t| t.spec().name == name).ok_or_else(|| anyhow::anyhow!("this node doesn't lend an MCP tool named '{name}'"))?;
                tool.call(args.get("arguments").cloned().unwrap_or_else(|| json!({}))).await
            }
            other => anyhow::bail!("this node has no '{other}'"),
        }
    }
}

/// What the log says about a call: the command, the path or the MCP tool.
fn activity_summary(tool: &str, args: &Value) -> String {
    let field = match tool {
        "shell" => "command",
        "mcp" => "tool",
        _ => "path",
    };
    args.get(field).and_then(Value::as_str).unwrap_or("").to_string()
}

fn read_file(vault: &Vault, path: &str) -> anyhow::Result<Value> {
    let full = vault.path_of(path)?;
    let size = std::fs::metadata(&full).with_context(|| format!("can't read '{path}'"))?.len();
    anyhow::ensure!(size <= MAX_READ_BYTES, "'{path}' is {size} bytes — over the {MAX_READ_BYTES}-byte limit for reading");
    let content = std::fs::read_to_string(&full).with_context(|| format!("can't read '{path}' as text"))?;
    Ok(json!({ "path": path, "content": content }))
}

fn list_files(vault: &Vault, path: &str) -> anyhow::Result<Value> {
    // An empty path is the whole folder; anything else has to be inside it too.
    let prefix = if path.is_empty() || path == "." { None } else { Some(vault.path_of(path)?.strip_prefix(vault.root())?.to_path_buf()) };
    let mut files: Vec<String> = vault
        .list_all_files()?
        .into_iter()
        .filter(|f| prefix.as_ref().is_none_or(|p| f.starts_with(p)))
        .map(|f| f.to_string_lossy().to_string())
        .collect();
    files.sort();
    let truncated = files.len() > MAX_LISTED_FILES;
    files.truncate(MAX_LISTED_FILES);
    Ok(json!({ "files": files, "truncated": truncated }))
}

/// Who this node is to the hub, kept between runs so the pairing key is only needed once.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NodeIdentity {
    pub device_id: String,
    #[serde(default)]
    pub device_token: Option<String>,
}

impl NodeIdentity {
    /// The saved identity, or a new id (`node-<name>-<8 hex>`) the first time.
    pub fn load_or_create(path: &Path, name: &str) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                use sha2::{Digest, Sha256};
                let seed = format!("{name}-{:?}-{}", std::time::SystemTime::now(), std::process::id());
                let suffix: String = Sha256::digest(seed.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect();
                let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).take(24).collect();
                let identity = Self { device_id: format!("node-{slug}-{suffix}"), device_token: None };
                identity.save(path)?;
                Ok(identity)
            }
            Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?).with_context(|| format!("failed to write {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

pub fn default_node_identity_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("node.json"))
}

/// Everything one connection needs.
pub struct NodeSession {
    pub hub_url: String,
    pub name: String,
    /// The hub's pairing key — only needed until the hub issues this node a token.
    pub auth_key: String,
    pub offer: NodeOfferDto,
    pub identity_path: Option<PathBuf>,
}

/// One connection to the hub, until it closes. Saves the token the hub issues.
pub async fn serve_once(session: &NodeSession, identity: &mut NodeIdentity, local: Arc<LocalNode>) -> anyhow::Result<()> {
    serve_once_watched(session, identity, local, None).await
}

/// `serve_once`, also saying `Connected` on `status` once the hub let it in.
async fn serve_once_watched(session: &NodeSession, identity: &mut NodeIdentity, local: Arc<LocalNode>, status: Option<&watch::Sender<NodeState>>) -> anyhow::Result<()> {
    let (mut conn, issued) = ServerConnection::handshake_full(
        &session.hub_url,
        &identity.device_id,
        &session.name,
        &session.auth_key,
        identity.device_token.clone(),
        Vec::new(),
        Some(session.offer.clone()),
        warden_server_protocol::tls::default_client_config(),
    )
    .await?;
    if let Some(token) = issued {
        identity.device_token = Some(token);
        if let Some(path) = &session.identity_path {
            identity.save(path)?;
        }
    }
    if let Some(status) = status {
        status.send_replace(NodeState::Connected);
    }
    eprintln!(
        "warden-server node: connected to {} as '{}' ({}). Agents can use it once it's approved in the hub's device list \
         and switched on for them (`warden-server nodes allow {}` on the hub, or the Devices screen).",
        session.hub_url, session.name, identity.device_id, identity.device_id
    );

    let (tx, mut rx) = mpsc::unbounded_channel::<ClientMessage>();
    // Model answers in flight, so a `ModelCancel` can stop one.
    let answering: Arc<std::sync::Mutex<HashMap<u64, tokio::task::AbortHandle>>> = Arc::default();
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.tick().await;
    let mut nonce = 0u64;
    loop {
        tokio::select! {
            msg = conn.recv() => match msg? {
                None => return Ok(()),
                Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
                    let (local, tx) = (local.clone(), tx.clone());
                    tokio::spawn(async move {
                        let reply = match local.run(&tool, arguments).await {
                            Ok(result) => ClientMessage::ToolCallResult { call_id, result },
                            Err(err) => ClientMessage::ToolCallError { call_id, message: format!("{err:#}") },
                        };
                        let _ = tx.send(reply);
                    });
                }
                Some(ServerMessage::ModelRequest { request_id, model, messages, tools }) => {
                    let (local, tx, done) = (local.clone(), tx.clone(), answering.clone());
                    let task = tokio::spawn(async move {
                        local.answer_model(request_id, &model, messages, tools, &tx).await;
                        done.lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id);
                    });
                    answering.lock().unwrap_or_else(|e| e.into_inner()).insert(request_id, task.abort_handle());
                }
                Some(ServerMessage::ModelCancel { request_id }) => {
                    if let Some(task) = answering.lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id) {
                        task.abort();
                    }
                }
                Some(ServerMessage::AuthError { reason }) => return Err(AuthRejected { reason: format!("the hub closed this node's access: {reason}") }.into()),
                Some(_) => {}
            },
            Some(out) = rx.recv() => conn.send(&out).await?,
            _ = heartbeat.tick() => {
                nonce += 1;
                conn.ping(nonce).await?;
            }
        }
    }
}

/// Keeps the node connected: reconnects with a growing wait (1 s up to a minute) whenever the hub
/// is away. Only returns on an error that retrying can't fix: a missing pairing key the first time,
/// or the hub turning this node away (a wrong key, a revoked token). `status`, when given, follows
/// each step.
pub async fn run_node(session: NodeSession, mut identity: NodeIdentity, local: Arc<LocalNode>, status: Option<watch::Sender<NodeState>>) -> anyhow::Result<()> {
    let set = |state: NodeState| {
        if let Some(status) = &status {
            status.send_replace(state);
        }
    };
    if identity.device_token.is_none() && session.auth_key.is_empty() {
        let error = "this node isn't paired with the hub yet — pass the hub's pairing key once with --auth-key (or WARDEN_SERVER_AUTH_KEY)";
        set(NodeState::Stopped { error: error.to_string() });
        anyhow::bail!(error);
    }
    let mut backoff = Duration::from_secs(1);
    loop {
        set(NodeState::Connecting);
        let error = match serve_once_watched(&session, &mut identity, local.clone(), status.as_ref()).await {
            Ok(()) => {
                eprintln!("warden-server node: the hub closed the connection, reconnecting");
                backoff = Duration::from_secs(1);
                "the hub closed the connection".to_string()
            }
            Err(err) if err.downcast_ref::<AuthRejected>().is_some() => {
                set(NodeState::Stopped { error: format!("{err:#}") });
                return Err(err);
            }
            Err(err) => {
                eprintln!("warden-server node: {err:#} — trying again in {}s", backoff.as_secs());
                format!("{err:#}")
            }
        };
        set(NodeState::Retrying { error, in_secs: backoff.as_secs() });
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Starts each MCP server named in `names` from the config at `config_path`, for a node to lend. Any
/// name that isn't there, or a server that doesn't start, stops the node from starting — lending half
/// of what was asked would be a surprise. `required`: a missing config file is an error.
pub async fn lend_mcp_servers(names: &[String], config_path: &Path, required: bool) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let file = warden_bootstrap::load_config_from_path(config_path, required)?;
    let mut tools = Vec::new();
    for name in names {
        let Some(server) = file.mcp_servers.iter().find(|s| s.name() == name) else {
            let known: Vec<&str> = file.mcp_servers.iter().map(|s| s.name()).collect();
            anyhow::bail!(
                "no MCP server named '{name}' in {} (it has: {})",
                config_path.display(),
                if known.is_empty() { "none".to_string() } else { known.join(", ") }
            );
        };
        let lent = warden_bootstrap::connect_mcp_server(server).await.with_context(|| format!("MCP server '{name}' didn't start"))?;
        warden_bootstrap::add_mcp_tools(&mut tools, name, Ok(lent));
    }
    Ok(tools)
}

/// Builds each model provider in `ids` from the config at `config_path` — like `lend_mcp_servers`, a
/// name that isn't there, or a provider that can't be built, stops the node from starting.
pub fn lend_models(ids: &[String], config_path: &Path, required: bool) -> anyhow::Result<Vec<(String, Arc<dyn ModelProvider>)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let file = warden_bootstrap::load_config_from_path(config_path, required)?;
    ids.iter()
        .map(|id| {
            let Some(provider) = file.providers.iter().find(|p| &p.id == id) else {
                let known: Vec<&str> = file.providers.iter().map(|p| p.id.as_str()).collect();
                anyhow::bail!("no provider named '{id}' in {} (it has: {})", config_path.display(), if known.is_empty() { "none".to_string() } else { known.join(", ") });
            };
            anyhow::ensure!(provider.kind != warden_bootstrap::Provider::Node, "provider '{id}' is itself another node's model — lend a local one");
            let built = warden_bootstrap::build_model_provider(provider, None).with_context(|| format!("provider '{id}' can't be used"))?;
            Ok((id.clone(), built))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warden-node-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn files_stay_inside_the_shared_folder() {
        let dir = temp_dir();
        let node = LocalNode::new(false, Some(dir.join("shared")));
        node.run("write_file", json!({ "path": "notes/a.txt", "content": "hi" })).await.unwrap();
        assert_eq!(node.run("read_file", json!({ "path": "notes/a.txt" })).await.unwrap()["content"], "hi");
        assert_eq!(node.run("list_files", json!({ "path": "notes" })).await.unwrap()["files"], json!(["notes/a.txt"]));
        std::fs::write(dir.join("secret.txt"), "no").unwrap();
        for path in ["../secret.txt", "/etc/passwd", "notes/../../secret.txt"] {
            assert!(node.run("read_file", json!({ "path": path })).await.is_err(), "{path}");
            assert!(node.run("write_file", json!({ "path": path, "content": "x" })).await.is_err(), "{path}");
        }
        assert!(node.run("list_files", json!({ "path": ".." })).await.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn what_isnt_lent_is_refused_here_too() {
        let files_only = LocalNode::new(false, Some(temp_dir()));
        assert!(files_only.run("shell", json!({ "command": "echo hi" })).await.is_err());
        let shell_only = LocalNode::new(true, None);
        assert!(shell_only.run("read_file", json!({ "path": "a" })).await.is_err());
        assert!(shell_only.run("format_disk", json!({})).await.is_err());
        let out = shell_only.run("shell", json!({ "command": "echo hello" })).await.unwrap();
        assert!(out["stdout"].as_str().unwrap().contains("hello"), "{out}");
        assert_eq!(shell_only.offer(String::new(), Vec::new()), NodeOfferDto { shell: true, files: false, ..NodeOfferDto::default() });
    }

    /// Stands in for an MCP server's tool: echoes its `text`.
    struct Echo;

    #[async_trait::async_trait]
    impl Tool for Echo {
        fn spec(&self) -> warden_core::tool::ToolSpec {
            warden_core::tool::ToolSpec { name: "echo_text".into(), description: "Echoes text".into(), parameters: json!({ "type": "object", "properties": { "text": { "type": "string" } } }) }
        }

        async fn call(&self, args: Value) -> anyhow::Result<Value> {
            Ok(json!({ "echoed": args["text"] }))
        }
    }

    #[tokio::test]
    async fn lent_mcp_tools_run_and_others_are_refused() {
        let node = LocalNode::new(false, None).with_mcp_tools(vec![Arc::new(Echo)]);
        assert_eq!(node.offer(String::new(), vec![]).mcp_tools[0].name, "echo_text");
        let out = node.run("mcp", json!({ "tool": "echo_text", "arguments": { "text": "hi" } })).await.unwrap();
        assert_eq!(out["echoed"], "hi");
        assert!(node.run("mcp", json!({ "tool": "rm_rf", "arguments": {} })).await.is_err());
        assert!(node.run("shell", json!({ "command": "echo x" })).await.is_err());
    }

    #[tokio::test]
    async fn the_activity_log_keeps_the_newest_calls_with_their_errors() {
        let activity = NodeActivity::default();
        let node = LocalNode::new(true, None).with_activity(activity.clone());
        node.run("shell", json!({ "command": "echo hi" })).await.unwrap();
        assert!(node.run("read_file", json!({ "path": "notes/a.txt" })).await.is_err());
        let log = activity.entries();
        assert_eq!((log[0].kind.as_str(), log[0].summary.as_str()), ("read_file", "notes/a.txt"));
        assert!(log[0].error.as_deref().unwrap().contains("doesn't share files"));
        assert_eq!((log[1].kind.as_str(), log[1].summary.as_str(), log[1].error.as_deref()), ("shell", "echo hi", None));
        for i in 0..MAX_ACTIVITY {
            activity.record("mcp", format!("tool-{i}"), None);
        }
        let log = activity.entries();
        assert_eq!(log.len(), MAX_ACTIVITY);
        assert_eq!(log[0].summary, format!("tool-{}", MAX_ACTIVITY - 1));
        assert!(log.iter().all(|e| e.kind == "mcp"), "the oldest calls went first");
    }

    #[tokio::test]
    async fn an_unpaired_node_without_a_key_stops_at_once() {
        let local = Arc::new(LocalNode::new(true, None));
        let session = NodeSession { hub_url: "ws://127.0.0.1:1".into(), name: "x".into(), auth_key: String::new(), offer: local.offer(String::new(), Vec::new()), identity_path: None };
        let (status, watching) = watch::channel(NodeState::Connecting);
        assert!(run_node(session, NodeIdentity::default(), local, Some(status)).await.is_err());
        assert!(matches!(&*watching.borrow(), NodeState::Stopped { error } if error.contains("isn't paired")));
    }

    #[test]
    fn the_identity_is_created_once_and_kept() {
        let dir = temp_dir();
        let path = dir.join("node.json");
        let first = NodeIdentity::load_or_create(&path, "Casa PC").unwrap();
        assert!(first.device_id.starts_with("node-casa-pc-"), "{}", first.device_id);
        assert_eq!(NodeIdentity::load_or_create(&path, "other").unwrap(), first);
        std::fs::remove_dir_all(&dir).ok();
    }
}
