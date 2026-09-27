//! Nodes (P93): machines running `warden-server node` that lend what they have (shell, a folder of
//! files) to the hub's agents. The state stays on the hub; a node only executes.
//!
//! Two locks, like the SSH hosts (P47): the node's operator chose what it offers (`--shell`,
//! `--files`), and the hub decides, per node, whether agents may use it, which ones, and whether every
//! call waits for a yes (`[[nodes]]` in `config.toml`). The node must also be approved in the device
//! list. This file keeps who is connected and answers the web's `ListNodes`/`SetNodeAccess`; the tools
//! the agents call are in `node_tools.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use warden_bootstrap::node_model::NodeModelRouter;
use warden_core::model::{ChatStream, Message, ProviderUnavailable, StreamEvent};
use warden_core::tool::ToolSpec;

use warden_bootstrap::{load_config_from_path, save_config, NodeAccessConfig};
use warden_server_protocol::protocol::{NodeInfoDto, NodeOfferDto};
use warden_server_protocol::ServerMessage;

use crate::device_registry::{PairingStatus, PairingStore};
use crate::remote_tool::RemoteToolChannel;
use crate::settings::{keys_match, SettingsHost, WRONG_KEY_DELAY};

/// A node that is connected right now.
#[derive(Clone)]
pub struct ConnectedNode {
    pub name: String,
    pub offer: NodeOfferDto,
    pub channel: RemoteToolChannel,
    /// Its model answers in flight (fatia 3).
    pub models: ModelChannel,
}

type ModelSink = mpsc::UnboundedSender<anyhow::Result<StreamEvent>>;

/// The model calls one node connection is answering: each `request_id` feeds one stream. The
/// connection's read loop delivers the node's `ModelEvent`/`ModelDone`/`ModelError`; when the
/// connection closes, every open stream fails at once.
#[derive(Clone)]
pub struct ModelChannel {
    tx: mpsc::WeakUnboundedSender<ServerMessage>,
    pending: Arc<Mutex<HashMap<u64, ModelSink>>>,
    next_id: Arc<AtomicU64>,
}

impl ModelChannel {
    /// A weak sender, so a stream left somewhere never keeps the connection's writer alive.
    pub fn new(tx: &mpsc::UnboundedSender<ServerMessage>) -> Self {
        Self { tx: tx.downgrade(), pending: Arc::default(), next_id: Arc::default() }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<u64, ModelSink>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sends the request and returns its stream's receiving end.
    fn open(&self, model: &str, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<(u64, mpsc::UnboundedReceiver<anyhow::Result<StreamEvent>>)> {
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sink, events) = mpsc::unbounded_channel();
        self.pending().insert(request_id, sink);
        let sent = self.tx.upgrade().is_some_and(|tx| tx.send(ServerMessage::ModelRequest { request_id, model: model.to_string(), messages, tools }).is_ok());
        if !sent {
            self.pending().remove(&request_id);
            return Err(ProviderUnavailable("the node disconnected".to_string()).into());
        }
        Ok((request_id, events))
    }

    /// One event from the node.
    pub fn deliver(&self, request_id: u64, event: StreamEvent) {
        if let Some(sink) = self.pending().get(&request_id) {
            let _ = sink.send(Ok(event));
        }
    }

    /// The answer ended: its stream closes.
    pub fn finish(&self, request_id: u64) {
        self.pending().remove(&request_id);
    }

    /// The node's model failed; a transient failure lets a combo move on.
    pub fn fail(&self, request_id: u64, message: String, transient: bool) {
        if let Some(sink) = self.pending().remove(&request_id) {
            let err = if transient { anyhow::Error::new(ProviderUnavailable(message)) } else { anyhow::anyhow!(message) };
            let _ = sink.send(Err(err));
        }
    }

    /// The connection closed: every answer still open fails now.
    pub fn close(&self, node_name: &str) {
        for (_, sink) in self.pending().drain() {
            let _ = sink.send(Err(ProviderUnavailable(format!("node '{node_name}' disconnected in the middle of the answer")).into()));
        }
    }

    /// The hub dropped a stream before it ended: tell the node to stop.
    fn cancel(&self, request_id: u64) {
        if self.pending().remove(&request_id).is_some() {
            if let Some(tx) = self.tx.upgrade() {
                let _ = tx.send(ServerMessage::ModelCancel { request_id });
            }
        }
    }
}

/// Holds a request open while its stream is alive; dropping it early cancels the request on the node.
struct CancelOnDrop {
    channel: ModelChannel,
    request_id: u64,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.channel.cancel(self.request_id);
    }
}

/// The hub's side of `warden_bootstrap::node_model` (fatia 3): a `kind = "node"` provider's call goes
/// to that node, when it's online, approved, switched on, open to the asking agent, and lends that
/// model. Otherwise the call fails as unavailable, so a combo moves on to its next provider.
pub struct HubNodeModelRouter {
    pub registry: NodeRegistry,
    pub config_path: PathBuf,
    pub devices_path: PathBuf,
}

#[async_trait]
impl NodeModelRouter for HubNodeModelRouter {
    async fn chat_stream(&self, node_id: &str, model: &str, agent: Option<&str>, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let unavailable = |why: String| -> anyhow::Error { ProviderUnavailable(why).into() };
        let node = self.registry.online().into_iter().find(|(id, _)| id == node_id).map(|(_, n)| n).ok_or_else(|| unavailable(format!("node '{node_id}' is offline")))?;
        if !matches!(PairingStore::new(self.devices_path.clone()).status(node_id), Ok(Some(PairingStatus::Approved))) {
            return Err(unavailable(format!("node '{node_id}' isn't approved in the device list")));
        }
        let config = load_config_from_path(&self.config_path, false)?;
        let access = config.nodes.iter().find(|n| n.id == node_id && n.enabled).ok_or_else(|| unavailable(format!("node '{node_id}' isn't switched on for agents")))?;
        if !access.agents.is_empty() && !agent.is_some_and(|a| access.agents.iter().any(|allowed| allowed == a)) {
            return Err(unavailable(format!("node '{node_id}' isn't open to {}", agent.map_or("chats without an agent".to_string(), |a| format!("agent '{a}'")))));
        }
        if !node.offer.models.iter().any(|m| m == model) {
            return Err(unavailable(format!("node '{node_id}' doesn't lend a model '{model}' (it lends: {})", if node.offer.models.is_empty() { "none".to_string() } else { node.offer.models.join(", ") })));
        }

        let (request_id, mut events) = node.models.open(model, messages, tools)?;
        let guard = CancelOnDrop { channel: node.models.clone(), request_id };
        // Waiting for the first event means a failure before any answer (the node's own model down)
        // still reaches `FallbackProvider` as an error it can move on from.
        let first = match events.recv().await {
            Some(Ok(event)) => Some(event),
            Some(Err(err)) => return Err(err),
            None => None,
        };
        let rest = futures_util::stream::unfold((events, guard), |(mut events, guard)| async move { events.recv().await.map(|item| (item, (events, guard))) });
        Ok(Box::pin(futures_util::stream::iter(first.map(Ok)).chain(rest)))
    }
}

#[derive(Default)]
struct Inner {
    online: HashMap<String, ConnectedNode>,
    /// Every node seen since the hub started, with what it offered last — so a screen can still show
    /// an offline one.
    seen: HashMap<String, (String, NodeOfferDto)>,
}

/// Who is connected as a node, shared by every connection and the node tools.
#[derive(Clone, Default)]
pub struct NodeRegistry(Arc<Mutex<Inner>>);

impl NodeRegistry {
    pub fn connect(&self, device_id: &str, node: ConnectedNode) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.seen.insert(device_id.to_string(), (node.name.clone(), node.offer.clone()));
        inner.online.insert(device_id.to_string(), node);
    }

    /// Takes the node offline — only if `channel` is still the connection registered for it, so an
    /// old connection closing after the node reconnected doesn't take the new one down.
    /// `true` when it did take one offline.
    pub fn disconnect(&self, device_id: &str, channel: &RemoteToolChannel) -> bool {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if inner.online.get(device_id).is_some_and(|n| n.channel.same(channel)) {
            inner.online.remove(device_id);
            return true;
        }
        false
    }

    pub fn online(&self) -> Vec<(String, ConnectedNode)> {
        let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut nodes: Vec<_> = inner.online.iter().map(|(id, n)| (id.clone(), n.clone())).collect();
        nodes.sort_by(|a, b| a.0.cmp(&b.0));
        nodes
    }

    fn seen(&self) -> HashMap<String, (String, NodeOfferDto)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).seen.clone()
    }
}

/// Every node the screens should show: connected or seen since the hub started, or named in `[[nodes]]`.
pub fn node_infos(registry: &NodeRegistry, pairing: &PairingStore, nodes: &[NodeAccessConfig]) -> Vec<NodeInfoDto> {
    let online: HashMap<String, ConnectedNode> = registry.online().into_iter().collect();
    let seen = registry.seen();
    let devices: HashMap<String, (String, PairingStatus)> = pairing.list().unwrap_or_default().into_iter().map(|(id, d)| (id, (d.device_name, d.status))).collect();

    let mut ids: Vec<String> = seen.keys().cloned().chain(nodes.iter().map(|n| n.id.clone())).collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|id| {
            let access = nodes.iter().find(|n| n.id == id);
            let (seen_name, offer) = seen.get(&id).cloned().map_or((None, None), |(name, offer)| (Some(name), Some(offer)));
            let device = devices.get(&id);
            NodeInfoDto {
                name: seen_name.or_else(|| device.map(|d| d.0.clone())).unwrap_or_else(|| id.clone()),
                online: online.contains_key(&id),
                approved: device.is_some_and(|d| d.1 == PairingStatus::Approved),
                offer,
                enabled: access.is_some_and(|a| a.enabled),
                agents: access.map(|a| a.agents.clone()).unwrap_or_default(),
                require_approval: access.is_some_and(|a| a.require_approval),
                device_id: id,
            }
        })
        .collect()
}

/// Writes `id`'s `[[nodes]]` entry, creating it the first time.
pub fn set_node_access(config_path: &Path, access: NodeAccessConfig) -> anyhow::Result<()> {
    anyhow::ensure!(!access.id.trim().is_empty(), "a node needs its device id");
    let mut config = load_config_from_path(config_path, false)?;
    for agent in &access.agents {
        anyhow::ensure!(config.agents.iter().any(|a| &a.id == agent), "there is no agent '{agent}'");
    }
    match config.nodes.iter_mut().find(|n| n.id == access.id) {
        Some(existing) => *existing = access,
        None => config.nodes.push(access),
    }
    save_config(config_path, &config)
}

fn node_error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::NodeError { request_id, message, auth_rejected }
}

fn list(registry: &NodeRegistry, pairing: &PairingStore, settings: Option<&dyn SettingsHost>, request_id: u64) -> anyhow::Result<ServerMessage> {
    let nodes = match settings {
        Some(host) => load_config_from_path(&host.config_path(), false)?.nodes,
        None => Vec::new(),
    };
    Ok(ServerMessage::NodeList { request_id, nodes: node_infos(registry, pairing, &nodes) })
}

/// Answers `ListNodes`.
pub fn handle_list_nodes(registry: &NodeRegistry, pairing: &PairingStore, settings: Option<&dyn SettingsHost>, request_id: u64) -> ServerMessage {
    list(registry, pairing, settings, request_id).unwrap_or_else(|err| node_error(request_id, format!("{err:#}"), false))
}

/// Answers `SetNodeAccess`: the pairing key again, with the same wait and lock as a settings save.
#[allow(clippy::too_many_arguments)]
pub async fn handle_set_node_access(
    registry: &NodeRegistry,
    pairing: &PairingStore,
    settings: Option<&dyn SettingsHost>,
    lock: &tokio::sync::Mutex<()>,
    auth_key: &str,
    request_id: u64,
    pairing_key: &str,
    access: NodeAccessConfig,
) -> ServerMessage {
    let Some(host) = settings else {
        return node_error(request_id, "this hub has no settings file, so it can't store node access".to_string(), false);
    };
    let _serialized = lock.lock().await;
    if !keys_match(pairing_key, auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return node_error(request_id, "wrong pairing key".to_string(), true);
    }
    match set_node_access(&host.config_path(), access).and_then(|()| list(registry, pairing, settings, request_id)) {
        Ok(reply) => reply,
        Err(err) => node_error(request_id, format!("{err:#}"), false),
    }
}
