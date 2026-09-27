//! Nodes (P93): machines running `warden-server node` that lend what they have (shell, a folder of
//! files) to the hub's agents. The state stays on the hub; a node only executes.
//!
//! Two locks, like the SSH hosts (P47): the node's operator chose what it offers (`--shell`,
//! `--files`), and the hub decides, per node, whether agents may use it, which ones, and whether every
//! call waits for a yes (`[[nodes]]` in `config.toml`). The node must also be approved in the device
//! list. This file keeps who is connected and answers the web's `ListNodes`/`SetNodeAccess`; the tools
//! the agents call are in `node_tools.rs`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

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
    pub fn disconnect(&self, device_id: &str, channel: &RemoteToolChannel) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if inner.online.get(device_id).is_some_and(|n| n.channel.same(channel)) {
            inner.online.remove(device_id);
        }
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
