//! Tauri commands backing the Workspace screen's "Lend this computer" (P97): the desktop running the
//! same node as `warden-server node` (`warden_server::node_client`) — connected to a hub (the one on
//! the VPS), lending its shell, a folder, MCP servers or models to that hub's agents.
//!
//! What's lent lives in `hub-local.json` (`HubLocalConfig::lend`), not in `config.toml`, which syncs
//! whole: switching this on here must not switch it on in every machine. The pairing key is never
//! saved — it's only needed until the hub issues a token, and that goes to `node.json`, the same
//! identity the CLI node uses.
//!
//! Lending to this machine's own embedded hub is refused: its agents already have this computer.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::watch;
use warden_bootstrap::tasks::{default_hub_local_path, load_hub_local, save_hub_local, LendConfig};
use warden_server::node_client::{
    default_node_identity_path, lend_mcp_servers, lend_models, run_node, ActivityEntry, LocalNode, NodeActivity, NodeIdentity, NodeSession, NodeState,
};

use crate::AppState;

/// The running node — dropped (and its task aborted) by `stop_lending`.
pub struct LendHandle {
    /// `None` when it never got going (a failed start at launch): only the state is left to show.
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    status: watch::Receiver<NodeState>,
    activity: NodeActivity,
}

impl LendHandle {
    pub(crate) fn stop(self) {
        if let Some(task) = self.task {
            // Drops the connection, the shell and the MCP servers' processes (`kill_on_drop`).
            task.abort();
        }
    }

    fn failed(error: String) -> Self {
        let (_, status) = watch::channel(NodeState::Stopped { error });
        Self { task: None, status, activity: NodeActivity::default() }
    }
}

/// The form, as the Workspace screen edits it.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LendConfigPayload {
    hub_url: String,
    name: String,
    description: String,
    tags: Vec<String>,
    shell: bool,
    files: Option<String>,
    mcp: Vec<String>,
    models: Vec<String>,
}

impl From<LendConfig> for LendConfigPayload {
    fn from(c: LendConfig) -> Self {
        Self {
            hub_url: c.hub_url,
            name: c.name,
            description: c.description,
            tags: c.tags,
            shell: c.shell,
            files: c.files.map(|f| f.to_string_lossy().to_string()),
            mcp: c.mcp,
            models: c.models,
        }
    }
}

impl LendConfigPayload {
    /// The form as it would be saved: trimmed, blanks dropped.
    fn into_config(self, enabled: bool) -> LendConfig {
        LendConfig {
            enabled,
            hub_url: self.hub_url.trim().to_string(),
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            tags: self.tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
            shell: self.shell,
            files: self.files.map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).map(PathBuf::from),
            mcp: self.mcp,
            models: self.models,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LendStatusPayload {
    /// The saved form — `None` if it was never set up.
    config: Option<LendConfigPayload>,
    /// The switch as saved: on means it comes back at launch.
    enabled: bool,
    /// Where the connection stands — `None` while off.
    state: Option<NodeState>,
    /// This computer's device id on the hub, once it has one.
    device_id: Option<String>,
    /// Holds a token from a hub, so the pairing key isn't needed again.
    paired: bool,
    /// Newest first.
    activity: Vec<ActivityEntry>,
}

/// What the form can pick from this machine's `config.toml`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LendOptionsPayload {
    mcp_servers: Vec<String>,
    /// Local providers only — a provider that is itself another node's model can't be lent on.
    models: Vec<String>,
    /// The host name, the name used when the form leaves it blank.
    default_name: String,
}

fn hub_local_path() -> Result<PathBuf, String> {
    default_hub_local_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

fn config_path() -> Result<PathBuf, String> {
    warden_bootstrap::default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

fn identity_path() -> Result<PathBuf, String> {
    default_node_identity_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

/// Saved identity, if this computer ever joined a hub as a node — reading never creates one.
fn saved_identity() -> Option<NodeIdentity> {
    let text = std::fs::read_to_string(identity_path().ok()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_lend(config: LendConfig) -> Result<(), String> {
    let path = hub_local_path()?;
    let mut local = load_hub_local(&path).map_err(|e| format!("{e:#}"))?;
    local.lend = Some(config);
    save_hub_local(&path, &local).map_err(|e| format!("{e:#}"))
}

/// `host:port` of a `ws://`/`wss://` URL, `None` for anything else.
fn url_host_port(url: &str) -> Option<(String, Option<u16>)> {
    let rest = url.strip_prefix("wss://").or_else(|| url.strip_prefix("ws://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    // `[::1]:7420` or `host:7420` or just `host`.
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']')?;
            (host.to_string(), after.strip_prefix(':').and_then(|p| p.parse().ok()))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), Some(port.parse().ok()?)),
            None => (authority.to_string(), None),
        },
    };
    Some((host.to_ascii_lowercase(), port))
}

/// Refuses a form that `warden-server node` would refuse, plus a hub URL that is this machine's own
/// embedded hub (`own_hub_port`).
fn check_lend_config(config: &LendConfig, own_hub_port: Option<u16>) -> Result<(), String> {
    let (host, port) = url_host_port(&config.hub_url).ok_or_else(|| "The hub address has to start with ws:// or wss://, like wss://my-vps.tailnet.ts.net:7420".to_string())?;
    let loopback = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if loopback && port.is_some() && port == own_hub_port {
        return Err("That's this computer's own hub — its agents already have this computer. Lend it to another hub, like the one on your VPS".to_string());
    }
    if !config.shell && config.files.is_none() && config.mcp.is_empty() && config.models.is_empty() {
        return Err("Pick something to lend: the shell, a folder, an MCP server or a model".to_string());
    }
    if let Some(dir) = &config.files {
        if !dir.is_dir() {
            return Err(format!("{} is not a folder", dir.display()));
        }
    }
    Ok(())
}

/// The port the embedded hub listens on (or would, once switched on).
fn own_hub_port(state: &AppState) -> Option<u16> {
    if let Some(hub) = state.embedded_server.lock().unwrap().as_ref() {
        return Some(hub.bound_addr.port());
    }
    warden_bootstrap::load_config_from_path(&config_path().ok()?, false).ok()?.embedded_server.map(|c| c.port)
}

/// Checks, starts what's lent (MCP servers, models) and spawns the node. `auth_key` is only needed
/// until the hub issues a token; a key given with a token already saved pairs again (the hub issues
/// a new one), which is how switching to another hub works.
pub(crate) async fn start_lending_inner(state: &AppState, config: &LendConfig, auth_key: String) -> Result<LendHandle, String> {
    check_lend_config(config, own_hub_port(state))?;
    let name = if config.name.is_empty() { warden_server::resolve_server_name(None) } else { config.name.clone() };
    let identity_path = identity_path()?;
    let identity = NodeIdentity::load_or_create(&identity_path, &name).map_err(|e| format!("{e:#}"))?;
    if identity.device_token.is_none() && auth_key.trim().is_empty() {
        return Err("This computer isn't paired with that hub yet — paste the hub's pairing key once".to_string());
    }
    let config_path = config_path()?;
    let mcp_tools = lend_mcp_servers(&config.mcp, &config_path, false).await.map_err(|e| format!("{e:#}"))?;
    let models = lend_models(&config.models, &config_path, false).map_err(|e| format!("{e:#}"))?;
    let activity = NodeActivity::default();
    let local = Arc::new(LocalNode::new(config.shell, config.files.clone()).with_mcp_tools(mcp_tools).with_models(models).with_activity(activity.clone()));
    let session = NodeSession {
        hub_url: config.hub_url.clone(),
        name,
        auth_key: auth_key.trim().to_string(),
        offer: local.offer(config.description.clone(), config.tags.clone()),
        identity_path: Some(identity_path),
    };
    let (sender, status) = watch::channel(NodeState::Connecting);
    let task = tauri::async_runtime::spawn(async move {
        if let Err(err) = run_node(session, identity, local, Some(sender)).await {
            eprintln!("desktop: lending this computer stopped: {err:#}");
        }
    });
    Ok(LendHandle { task: Some(task), status, activity })
}

/// At launch: brings back a node that was on when the app closed. A start that fails leaves its
/// reason on the screen instead of stopping the app from opening.
pub(crate) fn restore_lending(state: &AppState) {
    let Some(config) = default_hub_local_path().and_then(|path| load_hub_local(&path).ok()).and_then(|local| local.lend).filter(|lend| lend.enabled) else {
        return;
    };
    let handle = match tauri::async_runtime::block_on(start_lending_inner(state, &config, String::new())) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("desktop: failed to lend this computer again: {err}");
            LendHandle::failed(err)
        }
    };
    *state.lending.lock().unwrap() = Some(handle);
}

fn status(state: &AppState) -> Result<LendStatusPayload, String> {
    let saved = load_hub_local(&hub_local_path()?).map_err(|e| format!("{e:#}"))?.lend;
    let identity = saved_identity();
    let lending = state.lending.lock().unwrap();
    Ok(LendStatusPayload {
        enabled: saved.as_ref().is_some_and(|c| c.enabled),
        config: saved.map(LendConfigPayload::from),
        state: lending.as_ref().map(|h| h.status.borrow().clone()),
        device_id: identity.as_ref().map(|i| i.device_id.clone()),
        paired: identity.is_some_and(|i| i.device_token.is_some()),
        activity: lending.as_ref().map(|h| h.activity.entries()).unwrap_or_default(),
    })
}

#[tauri::command]
pub fn get_lend_status(state: State<'_, AppState>) -> Result<LendStatusPayload, String> {
    status(&state)
}

#[tauri::command]
pub fn lend_options() -> Result<LendOptionsPayload, String> {
    let config = warden_bootstrap::load_config_from_path(&config_path()?, false).map_err(|e| format!("{e:#}"))?;
    Ok(LendOptionsPayload {
        mcp_servers: config.mcp_servers.iter().map(|s| s.name().to_string()).collect(),
        models: config.providers.iter().filter(|p| p.kind != warden_bootstrap::Provider::Node).map(|p| p.id.clone()).collect(),
        default_name: warden_server::resolve_server_name(None),
    })
}

/// Stops what's running, starts with this form and saves it switched on. A form that can't start
/// isn't saved as on.
#[tauri::command]
pub async fn start_lending(state: State<'_, AppState>, config: LendConfigPayload, auth_key: Option<String>) -> Result<LendStatusPayload, String> {
    let config = config.into_config(true);
    if let Some(old) = state.lending.lock().unwrap().take() {
        old.stop();
    }
    let handle = start_lending_inner(&state, &config, auth_key.unwrap_or_default()).await?;
    *state.lending.lock().unwrap() = Some(handle);
    save_lend(config)?;
    status(&state)
}

/// Disconnects and saves the switch off; the form stays for next time.
#[tauri::command]
pub fn stop_lending(state: State<'_, AppState>) -> Result<LendStatusPayload, String> {
    if let Some(handle) = state.lending.lock().unwrap().take() {
        handle.stop();
    }
    let path = hub_local_path()?;
    let mut local = load_hub_local(&path).map_err(|e| format!("{e:#}"))?;
    if let Some(lend) = local.lend.as_mut() {
        lend.enabled = false;
        save_hub_local(&path, &local).map_err(|e| format!("{e:#}"))?;
    }
    status(&state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lend(hub_url: &str) -> LendConfig {
        LendConfig { hub_url: hub_url.into(), shell: true, ..LendConfig::default() }
    }

    #[test]
    fn hub_addresses_are_read_with_or_without_a_port() {
        assert_eq!(url_host_port("wss://vps.tail.ts.net:7420/"), Some(("vps.tail.ts.net".into(), Some(7420))));
        assert_eq!(url_host_port("ws://[::1]:7420"), Some(("::1".into(), Some(7420))));
        assert_eq!(url_host_port("ws://LocalHost"), Some(("localhost".into(), None)));
        assert_eq!(url_host_port("https://vps:7420"), None);
        assert_eq!(url_host_port("ws://"), None);
        assert_eq!(url_host_port("ws://host:notaport"), None);
    }

    #[test]
    fn the_form_refuses_what_the_cli_node_would_and_the_own_hub() {
        assert!(check_lend_config(&lend("wss://vps.tail.ts.net:7420"), Some(7420)).is_ok());
        assert!(check_lend_config(&lend("vps:7420"), None).unwrap_err().contains("ws://"));
        for own in ["ws://127.0.0.1:7420", "ws://localhost:7420", "wss://[::1]:7420"] {
            assert!(check_lend_config(&lend(own), Some(7420)).unwrap_err().contains("own hub"), "{own}");
        }
        // Another port on this machine is another hub (a `warden-server serve` next to the app).
        assert!(check_lend_config(&lend("ws://127.0.0.1:7421"), Some(7420)).is_ok());
        let nothing = LendConfig { shell: false, ..lend("wss://vps:7420") };
        assert!(check_lend_config(&nothing, None).unwrap_err().contains("Pick something"));
        let missing = LendConfig { files: Some(PathBuf::from("/no/such/folder/for/warden")), ..lend("wss://vps:7420") };
        assert!(check_lend_config(&missing, None).unwrap_err().contains("not a folder"));
    }

    #[test]
    fn saving_the_form_trims_and_drops_blanks() {
        let form = LendConfigPayload {
            hub_url: " wss://vps:7420 ".into(),
            name: "  ".into(),
            tags: vec![" home ".into(), " ".into()],
            files: Some("  ".into()),
            shell: true,
            ..LendConfigPayload::default()
        };
        let saved = form.into_config(true);
        assert_eq!((saved.hub_url.as_str(), saved.name.as_str(), saved.tags.clone(), saved.files.clone(), saved.enabled), ("wss://vps:7420", "", vec!["home".to_string()], None, true));
    }

    #[test]
    fn the_status_serializes_as_camel_case_with_a_tagged_state() {
        let payload = LendStatusPayload {
            config: None,
            enabled: true,
            state: Some(NodeState::Retrying { error: "down".into(), in_secs: 4 }),
            device_id: Some("node-desk-1".into()),
            paired: true,
            activity: Vec::new(),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["state"], serde_json::json!({ "state": "retrying", "error": "down", "inSecs": 4 }));
        assert_eq!(json["deviceId"], "node-desk-1");
    }
}
