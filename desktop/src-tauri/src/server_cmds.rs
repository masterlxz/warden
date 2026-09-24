//! Tauri commands backing the desktop app embedding its own `warden-server` hub (Fase 9.1
//! follow-up, "virar o hub desta rede") instead of that always being a separate process the
//! operator has to start by hand. Reuses the exact same `warden_server::Server`/paths the
//! standalone binary would — `AppState.orchestrator` (the same one the desktop's own chat already
//! uses) becomes the hub's `Orchestrator`, and `default_server_conversations_dir`/
//! `default_server_devices_path` (`warden-bootstrap`) are the same defaults `warden-server serve`
//! resolves to, so `WorkspaceView.tsx`'s existing device list/pairing UI works unmodified whether
//! the hub is this embedded one or a separate process on the same machine.
//!
//! `enabled` in the persisted `EmbeddedServerConfig` only ever flips via `start_embedded_server`/
//! `stop_embedded_server` — `save_embedded_server_config` (port/auth key/name) never touches it,
//! so editing those fields never silently turns the server on or off.

use std::net::SocketAddr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::oneshot;
use warden_bootstrap::{
    default_config_path, default_server_conversations_dir, default_server_devices_path, default_tls_dir, generate_auth_key, load_config_from_path, save_config,
    EmbeddedServerConfig,
};

use crate::AppState;

/// The running embedded server's shutdown handle — consumed (sending stops the accept loop) by
/// `stop_embedded_server`. `bound_addr`/`server_name` are read back by the Settings UI as status.
pub struct EmbeddedServerHandle {
    shutdown_tx: oneshot::Sender<()>,
    pub(crate) bound_addr: SocketAddr,
    server_name: String,
    /// `wss://` URL clients should use — `Some` only when started with the Tailscale cert (P36).
    secure_url: Option<String>,
    /// The daily `tailscale cert` renewal loop, aborted on stop so it doesn't outlive the hub.
    cert_renewal: Option<tokio::task::JoinHandle<()>>,
}

impl EmbeddedServerHandle {
    fn stop(self) {
        let _ = self.shutdown_tx.send(());
        if let Some(renewal) = self.cert_renewal {
            renewal.abort();
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedServerConfigPayload {
    port: u16,
    auth_key: String,
    server_name: Option<String>,
    tailscale_cert: bool,
}

impl From<EmbeddedServerConfig> for EmbeddedServerConfigPayload {
    fn from(c: EmbeddedServerConfig) -> Self {
        Self { port: c.port, auth_key: c.auth_key, server_name: c.server_name, tailscale_cert: c.tailscale_cert }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedServerStatusPayload {
    running: bool,
    bound_addr: Option<String>,
    server_name: Option<String>,
    secure_url: Option<String>,
    /// Where to open the hub's web interface (P78) from this machine — `None` when this build has
    /// no web UI compiled in (`web/` never built).
    web_url: Option<String>,
}

impl EmbeddedServerStatusPayload {
    fn stopped() -> Self {
        Self { running: false, bound_addr: None, server_name: None, secure_url: None, web_url: None }
    }

    fn running(handle: &EmbeddedServerHandle) -> Self {
        Self {
            running: true,
            bound_addr: Some(handle.bound_addr.to_string()),
            server_name: Some(handle.server_name.clone()),
            secure_url: handle.secure_url.clone(),
            web_url: web_ui_built().then(|| match &handle.secure_url {
                Some(url) => url.replacen("wss://", "https://", 1),
                None => format!("http://localhost:{}", handle.bound_addr.port()),
            }),
        }
    }
}

fn web_ui_built() -> bool {
    use warden_server::WebAssets;
    warden_server::EmbeddedWebUi.get("index.html").is_some()
}

fn config_path() -> Result<std::path::PathBuf, String> {
    default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

/// `None` when never configured — the Settings UI shows a freshly generated port/key suggestion
/// in that case (via `generate_embedded_server_auth_key`) instead of an empty, invalid form.
#[tauri::command]
pub fn get_embedded_server_config() -> Result<Option<EmbeddedServerConfigPayload>, String> {
    let path = config_path()?;
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    Ok(config.embedded_server.map(Into::into))
}

/// A fresh random secret for the "Auth key" field — never hand-typed, see `generate_auth_key`'s
/// own doc comment for why.
#[tauri::command]
pub fn generate_embedded_server_auth_key() -> String {
    generate_auth_key()
}

#[tauri::command]
pub fn save_embedded_server_config(port: u16, auth_key: String, server_name: Option<String>, tailscale_cert: bool) -> Result<(), String> {
    let auth_key = auth_key.trim().to_string();
    if auth_key.is_empty() {
        return Err("Auth key não pode ficar em branco".to_string());
    }
    if port == 0 {
        return Err("Escolha uma porta válida".to_string());
    }
    let path = config_path()?;
    let mut config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    let enabled = config.embedded_server.as_ref().is_some_and(|c| c.enabled);
    config.embedded_server =
        Some(EmbeddedServerConfig { enabled, port, auth_key, server_name: server_name.filter(|n| !n.trim().is_empty()), tailscale_cert });
    save_config(&path, &config).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn start_embedded_server(state: State<'_, AppState>) -> Result<EmbeddedServerStatusPayload, String> {
    let path = config_path()?;
    let mut config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    let server_config = config.embedded_server.clone().ok_or_else(|| "configure a porta e a auth key antes de ligar".to_string())?;

    let handle = start_embedded_server_inner(&state, &server_config).await.map_err(|e| format!("{e:#}"))?;
    let status = EmbeddedServerStatusPayload::running(&handle);
    *state.embedded_server.lock().unwrap() = Some(handle);

    config.embedded_server = Some(EmbeddedServerConfig { enabled: true, ..server_config });
    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;

    Ok(status)
}

#[tauri::command]
pub fn stop_embedded_server(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(handle) = state.embedded_server.lock().unwrap().take() {
        handle.stop();
    }

    let path = config_path()?;
    let mut config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    if let Some(server_config) = config.embedded_server.clone() {
        config.embedded_server = Some(EmbeddedServerConfig { enabled: false, ..server_config });
        save_config(&path, &config).map_err(|e| format!("{e:#}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn embedded_server_status(state: State<'_, AppState>) -> EmbeddedServerStatusPayload {
    match state.embedded_server.lock().unwrap().as_ref() {
        Some(handle) => EmbeddedServerStatusPayload::running(handle),
        None => EmbeddedServerStatusPayload::stopped(),
    }
}

/// Shared by `start_embedded_server` and `run()`'s auto-start-on-launch (Fase 9.1 follow-up) —
/// builds and spawns the real `warden_server::Server`, wired to `serve_until` so the returned
/// handle's `shutdown_tx` actually stops it and frees the port later.
pub(crate) async fn start_embedded_server_inner(state: &AppState, config: &EmbeddedServerConfig) -> anyhow::Result<EmbeddedServerHandle> {
    let orchestrator = state.orchestrator.lock().unwrap().clone().map_err(|e| anyhow::anyhow!(e))?;
    let conversations_dir = default_server_conversations_dir().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?;
    let devices_path = default_server_devices_path().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?;
    let server_name = warden_server::resolve_server_name(config.server_name.clone());
    let addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;

    // P36: fetched before binding, so a Tailscale problem (not installed, HTTPS certs off) fails
    // the start with its own message instead of leaving a half-configured hub running.
    let tailscale = if config.tailscale_cert {
        let dir = default_tls_dir().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?;
        Some(warden_server::HubTls::from_tailscale(&dir).await?)
    } else {
        None
    };

    let mut server = warden_server::Server::bind(addr, config.auth_key.clone(), server_name.clone(), Arc::new(orchestrator), conversations_dir, devices_path)
        .await?
        .with_web_ui(Arc::new(warden_server::EmbeddedWebUi));
    let bound_addr = server.local_addr()?;
    let (secure_url, cert_renewal) = match tailscale {
        Some((tls, cert)) => {
            let secure_url = tls.secure_url(bound_addr.port());
            server = server.with_tls(tls);
            (secure_url, Some(tokio::spawn(cert.renewal())))
        }
        None => (None, None),
    };
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(server.serve_until(async {
        let _ = shutdown_rx.await;
    }));
    Ok(EmbeddedServerHandle { shutdown_tx, bound_addr, server_name, secure_url, cert_renewal })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not mocked: a real `Orchestrator` (via `bootstrap()`, pointed at a throwaway config with a
    /// fake-but-present API key — never actually calls the model, so the fake key is never
    /// exercised), a real `AppState`, a real `Server::bind`/`serve_until`, and a real client
    /// (`ServerConnection`) connecting over an actual TCP socket. Proves the whole embedded-server
    /// wiring — not just that the code compiles — the same bar the three earlier discovery slices
    /// held themselves to this session.
    #[tokio::test]
    async fn start_embedded_server_inner_binds_and_answers_real_hello_and_discover() {
        let temp_dir = std::env::temp_dir().join(format!(
            "desktop-embedded-server-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let config_path = temp_dir.join("config.toml");
        warden_bootstrap::save_config(
            &config_path,
            &warden_bootstrap::FileConfig {
                api_keys: warden_bootstrap::ApiKeys { gemini: Some("fake-key-for-test".to_string()), ..Default::default() },
                ..Default::default()
            },
        )
        .unwrap();

        let orchestrator = warden_bootstrap::bootstrap(
            Some(config_path.to_str().unwrap()),
            warden_bootstrap::Overrides { provider: Some(warden_bootstrap::Provider::Gemini), ..Default::default() },
            temp_dir.join("vault"),
        )
        .await
        .unwrap();

        let sync = warden_sync::SyncEngine::new(temp_dir.join("vault"), config_path, temp_dir.join("secrets.json"), temp_dir.join("manifest.json"));

        let state = crate::AppState {
            orchestrator: std::sync::Mutex::new(Ok(orchestrator)),
            recording: std::sync::Mutex::new(None),
            sync,
            pending_push: std::sync::Mutex::new(None),
            generated_files_root: temp_dir.join("generated"),
            embedded_server: std::sync::Mutex::new(None),
            approvals: std::sync::Arc::new(crate::approval::ApprovalBroker::default()),
        };

        let server_config =
            EmbeddedServerConfig { enabled: true, port: 0, auth_key: "test-auth-key".to_string(), server_name: Some("Test Desktop".to_string()), tailscale_cert: false };

        let handle = start_embedded_server_inner(&state, &server_config).await.unwrap();
        // `bound_addr` reflects the `0.0.0.0` bind host `local_addr()` reports — not a connectable
        // destination on every platform, so dial loopback explicitly with the same assigned port.
        let port = handle.bound_addr.port();

        let mut conn = warden_server::ServerConnection::connect(&format!("ws://127.0.0.1:{port}"), "dev-1", "Test Client", "test-auth-key")
            .await
            .unwrap();
        conn.ping(1).await.unwrap();
        assert!(matches!(conn.recv().await.unwrap(), Some(warden_server::ServerMessage::Pong { nonce: 1 })));

        let hubs = warden_server::discover_hubs_on(vec![std::net::Ipv4Addr::LOCALHOST], port).await.unwrap();
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].server_name, "Test Desktop");
        assert_eq!(hubs[0].secure_url, None);
        assert_eq!(handle.secure_url, None);

        handle.stop();
    }

    // Locks in the exact camelCase JSON shape `desktop/src/types.ts` expects.
    #[test]
    fn embedded_server_config_payload_serializes_as_camel_case() {
        let payload = EmbeddedServerConfigPayload::from(EmbeddedServerConfig {
            enabled: true,
            port: 7420,
            auth_key: "secret".to_string(),
            server_name: None,
            tailscale_cert: true,
        });
        assert_eq!(serde_json::to_string(&payload).unwrap(), r#"{"port":7420,"authKey":"secret","serverName":null,"tailscaleCert":true}"#);
    }

    #[test]
    fn stopped_status_serializes_as_camel_case() {
        assert_eq!(
            serde_json::to_string(&EmbeddedServerStatusPayload::stopped()).unwrap(),
            r#"{"running":false,"boundAddr":null,"serverName":null,"secureUrl":null,"webUrl":null}"#
        );
    }
}
