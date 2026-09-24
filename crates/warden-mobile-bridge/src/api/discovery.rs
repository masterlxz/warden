//! Fase 9.1 (redefined) bridge function for the mobile `ConnectionScreen` — reuses the exact same
//! LAN sweep the desktop's `WorkspaceView.tsx` calls (`warden_server_protocol::discover_hubs`),
//! not a Dart reimplementation. Same "plain `pub fn`, own Tokio runtime" shape as `api::sync`'s
//! bridge functions — see that module's docs for why.

use std::sync::OnceLock;

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("failed to start Tokio runtime for warden_mobile_bridge"))
}

#[derive(Debug, Clone)]
pub struct DiscoveredHubDto {
    pub host: String,
    pub port: u16,
    pub server_name: String,
    /// Set when the hub only accepts `wss://` (P36) — where to connect instead of `ws://host:port`.
    pub secure_url: Option<String>,
}

/// Sweeps the local network for `warden-server` hubs listening on `port` — same mechanism as
/// `desktop/src-tauri/src/workspace_cmds.rs::discover_hubs`. Never needs or reveals the auth key;
/// the `ConnectionScreen` still asks for that by hand, same security boundary as the desktop.
pub fn bridge_discover_hubs(port: u16) -> Result<Vec<DiscoveredHubDto>, String> {
    rt().block_on(warden_server_protocol::discover_hubs(port))
        .map(|hubs| {
            hubs.into_iter()
                .map(|h| DiscoveredHubDto { host: h.host.to_string(), port: h.port, server_name: h.server_name, secure_url: h.secure_url })
                .collect()
        })
        .map_err(|e| format!("{e:#}"))
}
