//! Tauri commands backing the Workspace screen's "Nodes" section (P93): the machines lending their
//! shell or a folder to the hub's agents, and the `[[nodes]]` entries that say who may use them. The
//! entries live in `config.toml`, which syncs, so allowing a node here also allows it on the hub in
//! the VPS once the file gets there. Who's online is only known for this machine's embedded hub.

use tauri::State;
use warden_server::nodes::{node_infos, set_node_access};
use warden_server::PairingStore;
use warden_server_protocol::protocol::NodeInfoDto;

use crate::AppState;

fn config_path() -> Result<std::path::PathBuf, String> {
    warden_bootstrap::default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

#[tauri::command]
pub fn list_nodes(state: State<'_, AppState>) -> Result<Vec<NodeInfoDto>, String> {
    let registry = state.embedded_server.lock().unwrap().as_ref().map(|hub| hub.node_registry.clone()).unwrap_or_default();
    let devices_path = warden_bootstrap::default_server_devices_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let config = warden_bootstrap::load_config_from_path(&config_path()?, false).map_err(|e| format!("{e:#}"))?;
    Ok(node_infos(&registry, &PairingStore::new(devices_path), &config.nodes))
}

/// Writes one node's access — also for a node that joins another hub, typed in by its device id.
#[tauri::command]
pub fn save_node_access(state: State<'_, AppState>, device_id: String, enabled: bool, agents: Vec<String>, require_approval: bool) -> Result<Vec<NodeInfoDto>, String> {
    let access = warden_bootstrap::NodeAccessConfig { id: device_id.trim().to_string(), enabled, agents, require_approval };
    set_node_access(&config_path()?, access).map_err(|e| format!("{e:#}"))?;
    list_nodes(state)
}
