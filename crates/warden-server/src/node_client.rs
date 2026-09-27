//! The node side of P93: `warden-server node --hub wss://…` connects to a hub as one more device,
//! announces what this machine lends (a shell, a folder of files) and runs the calls the hub's agents
//! make on it. Nothing else lives here: no vault, no agents, no model — the state stays on the hub.
//!
//! This is the node's own lock of the two: whatever wasn't switched on here (`--shell`, `--files`) is
//! refused here too, whatever the hub asks. Files never leave the chosen folder (`Vault::path_of`
//! refuses absolute paths and `..`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use std::collections::HashMap;

use futures_util::StreamExt;
use warden_core::memory::Vault;
use warden_core::model::fallback::is_transient;
use warden_core::model::{Message, ModelProvider, StreamEvent};
use warden_core::tool::ToolSpec;
use warden_core::tool::shell::ShellTool;
use warden_core::tool::Tool;
use warden_server_protocol::protocol::NodeOfferDto;
use warden_server_protocol::{ClientMessage, ServerConnection, ServerMessage};

const HEARTBEAT: Duration = Duration::from_secs(20);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Biggest file `read_file` sends back: this is text for a model, not a transfer tool.
const MAX_READ_BYTES: u64 = 1024 * 1024;
const MAX_LISTED_FILES: usize = 1000;

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
}

impl LocalNode {
    /// `shell`: run commands (in `files`' folder, or the home directory). `files`: the folder shared.
    pub fn new(shell: bool, files: Option<PathBuf>) -> Self {
        let base = files.clone().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
        Self { shell: shell.then(|| ShellTool::new(Arc::new(Vault::new(base)))), files: files.map(|dir| Arc::new(Vault::new(dir))), mcp: Vec::new(), models: Vec::new() }
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
    pub async fn answer_model(&self, request_id: u64, model: &str, messages: Vec<Message>, tools: Vec<ToolSpec>, reply: &mpsc::UnboundedSender<ClientMessage>) {
        let Some((_, provider)) = self.models.iter().find(|(id, _)| id == model) else {
            let _ = reply.send(ClientMessage::ModelError { request_id, message: format!("this node doesn't lend a model '{model}'"), transient: false });
            return;
        };
        let mut stream = match provider.chat_stream(messages, tools).await {
            Ok(stream) => stream,
            Err(err) => {
                let _ = reply.send(ClientMessage::ModelError { request_id, message: format!("{err:#}"), transient: is_transient(&err) });
                return;
            }
        };
        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamEvent::ProviderFallback(_)) => {}
                Ok(event) => {
                    if reply.send(ClientMessage::ModelEvent { request_id, event }).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    let _ = reply.send(ClientMessage::ModelError { request_id, message: format!("{err:#}"), transient: false });
                    return;
                }
            }
        }
        let _ = reply.send(ClientMessage::ModelDone { request_id });
    }

    /// Runs one call from the hub: `shell`, `read_file`, `write_file` or `list_files`.
    pub async fn run(&self, tool: &str, args: Value) -> anyhow::Result<Value> {
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
                Some(ServerMessage::AuthError { reason }) => anyhow::bail!("the hub closed this node's access: {reason}"),
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
/// is away. Only returns on an error that retrying can't fix — a missing pairing key the first time.
pub async fn run_node(session: NodeSession, mut identity: NodeIdentity, local: Arc<LocalNode>) -> anyhow::Result<()> {
    anyhow::ensure!(
        identity.device_token.is_some() || !session.auth_key.is_empty(),
        "this node isn't paired with the hub yet — pass the hub's pairing key once with --auth-key (or WARDEN_SERVER_AUTH_KEY)"
    );
    let mut backoff = Duration::from_secs(1);
    loop {
        match serve_once(&session, &mut identity, local.clone()).await {
            Ok(()) => {
                eprintln!("warden-server node: the hub closed the connection, reconnecting");
                backoff = Duration::from_secs(1);
            }
            Err(err) => eprintln!("warden-server node: {err:#} — trying again in {}s", backoff.as_secs()),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
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
