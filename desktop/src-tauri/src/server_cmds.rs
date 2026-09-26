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

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::oneshot;
use warden_bootstrap::{
    default_config_path, default_server_conversations_dir, default_server_devices_path, default_tls_dir, generate_auth_key, is_strong_auth_key, load_config_from_path, save_config,
    EmbeddedServerConfig, MIN_AUTH_KEY_LEN,
};
use warden_core::orchestrator::Orchestrator;
use warden_server::{SettingsHost, SharedOrchestrator};

use crate::AppState;

/// The running embedded server's shutdown handle — consumed (sending stops the accept loop) by
/// `stop_embedded_server`. `bound_addr`/`server_name` are read back by the Settings UI as status.
pub struct EmbeddedServerHandle {
    shutdown_tx: oneshot::Sender<()>,
    pub(crate) bound_addr: SocketAddr,
    server_name: String,
    /// `wss://` URL clients should use — `Some` when started with TLS and a known host name (P36).
    secure_url: Option<String>,
    /// TLS without a known host name (a certificate with no `tls_host`): the web link can't be built.
    tls_without_host: bool,
    web_ui: bool,
    /// The daily `tailscale cert` renewal loop, aborted on stop so it doesn't outlive the hub.
    cert_renewal: Option<tokio::task::JoinHandle<()>>,
    /// The hub's orchestrator — replaced when the desktop's own Settings save (P78), so the hub
    /// runs on the new settings without a restart.
    pub(crate) orchestrator: SharedOrchestrator,
}

/// The embedded hub's web settings (P78): the desktop's own config file, built the way the desktop
/// builds its orchestrator, and handed to the desktop's chat as well once a save puts it in place.
struct DesktopHubSettings {
    config_path: PathBuf,
    desktop: Arc<Mutex<Result<Orchestrator, String>>>,
}

#[async_trait::async_trait]
impl SettingsHost for DesktopHubSettings {
    fn config_path(&self) -> PathBuf {
        self.config_path.clone()
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        warden_bootstrap::bootstrap(None, warden_bootstrap::Overrides::default(), crate::desktop_default_vault_path()).await
    }

    fn installed(&self, orchestrator: &Orchestrator) {
        *self.desktop.lock().unwrap() = Ok(orchestrator.clone());
    }
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
    listen_host: Option<String>,
    auth_key: String,
    server_name: Option<String>,
    tailscale_cert: bool,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    tls_host: Option<String>,
    web_ui: bool,
}

impl From<EmbeddedServerConfig> for EmbeddedServerConfigPayload {
    fn from(c: EmbeddedServerConfig) -> Self {
        Self {
            port: c.port,
            listen_host: c.listen_host,
            auth_key: c.auth_key,
            server_name: c.server_name,
            tailscale_cert: c.tailscale_cert,
            tls_cert: c.tls_cert,
            tls_key: c.tls_key,
            tls_host: c.tls_host,
            web_ui: c.web_ui,
        }
    }
}

fn non_blank(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl EmbeddedServerConfigPayload {
    /// The form as it would be saved: blanks become `None`, then the same rules `warden-server
    /// serve`'s flags have (a valid address, TLS files together and not with Tailscale, a strong key).
    fn into_config(self, enabled: bool) -> Result<EmbeddedServerConfig, String> {
        let config = EmbeddedServerConfig {
            enabled,
            port: self.port,
            listen_host: non_blank(self.listen_host),
            auth_key: self.auth_key.trim().to_string(),
            server_name: non_blank(self.server_name),
            tailscale_cert: self.tailscale_cert,
            tls_cert: non_blank(self.tls_cert),
            tls_key: non_blank(self.tls_key),
            tls_host: non_blank(self.tls_host),
            web_ui: self.web_ui,
        };
        if config.port == 0 {
            return Err("Escolha uma porta válida".to_string());
        }
        check_embedded_server_config(&config)?;
        Ok(config)
    }
}

/// Also run on every start, so a `config.toml` edited by hand fails with the same message.
fn check_embedded_server_config(config: &EmbeddedServerConfig) -> Result<(), String> {
    if config.auth_key.is_empty() {
        return Err("Auth key não pode ficar em branco".to_string());
    }
    if !is_strong_auth_key(&config.auth_key) {
        return Err(weak_auth_key_message());
    }
    if let Some(host) = &config.listen_host {
        host.parse::<IpAddr>().map_err(|_| format!("\"{host}\" não é um endereço IP (ex.: 0.0.0.0, 127.0.0.1 ou 192.168.1.10)"))?;
    }
    match (&config.tls_cert, &config.tls_key) {
        (Some(_), None) | (None, Some(_)) => return Err("Informe o certificado e a chave privada juntos".to_string()),
        (Some(_), Some(_)) if config.tailscale_cert => {
            return Err("Escolha um só: HTTPS via Tailscale ou certificado próprio".to_string());
        }
        (None, None) if config.tls_host.is_some() => return Err("O nome do certificado só vale com um certificado próprio".to_string()),
        _ => {}
    }
    Ok(())
}

fn listen_addr(config: &EmbeddedServerConfig) -> anyhow::Result<SocketAddr> {
    let host: IpAddr = config.listen_host.as_deref().unwrap_or("0.0.0.0").parse()?;
    Ok(SocketAddr::new(host, config.port))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedServerStatusPayload {
    running: bool,
    bound_addr: Option<String>,
    server_name: Option<String>,
    /// TLS-only, whether or not `secure_url` could be built.
    secure: bool,
    secure_url: Option<String>,
    /// Where to open the hub's web interface (P78) from this machine — `None` when it's turned
    /// off, this build has no web UI compiled in (`web/` never built), or TLS runs with no known
    /// host name to put in the link.
    web_url: Option<String>,
}

impl EmbeddedServerStatusPayload {
    fn stopped() -> Self {
        Self { running: false, bound_addr: None, server_name: None, secure: false, secure_url: None, web_url: None }
    }

    fn running(handle: &EmbeddedServerHandle) -> Self {
        Self {
            running: true,
            bound_addr: Some(handle.bound_addr.to_string()),
            server_name: Some(handle.server_name.clone()),
            secure: handle.secure_url.is_some() || handle.tls_without_host,
            secure_url: handle.secure_url.clone(),
            web_url: (handle.web_ui && web_ui_built() && !handle.tls_without_host).then(|| match &handle.secure_url {
                Some(url) => url.replacen("wss://", "https://", 1),
                None if handle.bound_addr.ip().is_unspecified() => format!("http://localhost:{}", handle.bound_addr.port()),
                None => format!("http://{}", handle.bound_addr),
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

fn weak_auth_key_message() -> String {
    format!("A auth key precisa ter pelo menos {MIN_AUTH_KEY_LEN} caracteres — use \"Gerar nova chave\" para criar uma nova (devices já pareados continuam funcionando)")
}

#[tauri::command]
pub fn save_embedded_server_config(config: EmbeddedServerConfigPayload) -> Result<(), String> {
    let path = config_path()?;
    let mut file = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    let enabled = file.embedded_server.as_ref().is_some_and(|c| c.enabled);
    file.embedded_server = Some(config.into_config(enabled)?);
    save_config(&path, &file).map_err(|e| format!("{e:#}"))
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
    // P83 — also catches a short key saved before this check existed, on start and on auto-start;
    // the rest catches a config.toml edited by hand into something the form wouldn't save.
    check_embedded_server_config(config).map_err(|message| anyhow::anyhow!(message))?;
    let orchestrator = state.orchestrator.lock().unwrap().clone().map_err(|e| anyhow::anyhow!(e))?;
    let conversations_dir = default_server_conversations_dir().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?;
    let devices_path = default_server_devices_path().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?;
    let server_name = warden_server::resolve_server_name(config.server_name.clone());
    let addr = listen_addr(config)?;

    // P36: fetched/loaded before binding, so a Tailscale problem (not installed, HTTPS certs off)
    // or a bad certificate file fails the start with its own message instead of leaving a
    // half-configured hub running. Same two sources as `warden-server serve`'s `resolve_tls`.
    let (tls, tailscale_cert) = if config.tailscale_cert {
        let dir = default_tls_dir().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?;
        let (tls, cert) = warden_server::HubTls::from_tailscale(&dir).await?;
        (Some(tls), Some(cert))
    } else if let (Some(cert), Some(key)) = (&config.tls_cert, &config.tls_key) {
        (Some(warden_server::HubTls::from_pem_files(cert, key, config.tls_host.clone())?), None)
    } else {
        (None, None)
    };

    let shared = SharedOrchestrator::new(orchestrator);
    let mut server = warden_server::Server::bind(addr, config.auth_key.clone(), server_name.clone(), shared.clone(), conversations_dir, devices_path)
        .await?
        // P78 — voice input from the web UI, with the same Whisper key as the desktop's mic button.
        .with_transcriber(Arc::new(warden_server::chat_input::WhisperTranscriber::new(None)));
    if config.web_ui {
        server = server.with_web_ui(Arc::new(warden_server::EmbeddedWebUi));
    }
    if let Some(config_path) = default_config_path() {
        server = server.with_settings(Arc::new(DesktopHubSettings { config_path, desktop: state.orchestrator.clone() }));
    }
    let bound_addr = server.local_addr()?;
    let secure_url = tls.as_ref().and_then(|tls| tls.secure_url(bound_addr.port()));
    let tls_without_host = tls.is_some() && secure_url.is_none();
    if let Some(tls) = tls {
        server = server.with_tls(tls);
    }
    let cert_renewal = tailscale_cert.map(|cert| tokio::spawn(cert.renewal()));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(server.serve_until(async {
        let _ = shutdown_rx.await;
    }));
    Ok(EmbeddedServerHandle { shutdown_tx, bound_addr, server_name, secure_url, tls_without_host, web_ui: config.web_ui, cert_renewal, orchestrator: shared })
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
            orchestrator: Arc::new(std::sync::Mutex::new(Ok(orchestrator))),
            recording: std::sync::Mutex::new(None),
            sync,
            pending_push: std::sync::Mutex::new(None),
            generated_files_root: temp_dir.join("generated"),
            embedded_server: std::sync::Mutex::new(None),
            approvals: std::sync::Arc::new(crate::approval::ApprovalBroker::default()),
        };

        let server_config =
            EmbeddedServerConfig { enabled: true, server_name: Some("Test Desktop".to_string()), ..EmbeddedServerConfig::new(0, "test-auth-key-long-enough-for-p83-check") };

        let handle = start_embedded_server_inner(&state, &server_config).await.unwrap();
        // `bound_addr` reflects the `0.0.0.0` bind host `local_addr()` reports — not a connectable
        // destination on every platform, so dial loopback explicitly with the same assigned port.
        let port = handle.bound_addr.port();

        let mut conn = warden_server::ServerConnection::connect(&format!("ws://127.0.0.1:{port}"), "dev-1", "Test Client", "test-auth-key-long-enough-for-p83-check")
            .await
            .unwrap();
        conn.ping(1).await.unwrap();
        assert!(matches!(conn.recv().await.unwrap(), Some(warden_server::ServerMessage::Pong { nonce: 1 })));

        let hubs = warden_server::discover_hubs_on(vec![std::net::Ipv4Addr::LOCALHOST], port, std::time::Duration::from_secs(10)).await.unwrap();
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].server_name, "Test Desktop");
        assert_eq!(hubs[0].secure_url, None);
        assert_eq!(handle.secure_url, None);

        handle.stop();
    }

    // Locks in the exact camelCase JSON shape `desktop/src/types.ts` expects.
    #[test]
    fn embedded_server_config_payload_serializes_as_camel_case() {
        let payload = EmbeddedServerConfigPayload::from(EmbeddedServerConfig { enabled: true, tailscale_cert: true, ..EmbeddedServerConfig::new(7420, "secret") });
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            r#"{"port":7420,"listenHost":null,"authKey":"secret","serverName":null,"tailscaleCert":true,"tlsCert":null,"tlsKey":null,"tlsHost":null,"webUi":true}"#
        );
    }

    fn form() -> EmbeddedServerConfigPayload {
        EmbeddedServerConfig::new(7420, "k".repeat(64)).into()
    }

    #[test]
    fn saving_the_form_trims_blanks_and_keeps_every_serve_flag() {
        let payload = EmbeddedServerConfigPayload {
            listen_host: Some(" 127.0.0.1 ".into()),
            server_name: Some("  ".into()),
            tls_cert: Some("/etc/hub/cert.pem".into()),
            tls_key: Some("/etc/hub/key.pem".into()),
            tls_host: Some("".into()),
            web_ui: false,
            ..form()
        };
        let config = payload.into_config(true).unwrap();
        assert_eq!(config.listen_host.as_deref(), Some("127.0.0.1"));
        assert_eq!((config.server_name.as_deref(), config.tls_host.as_deref()), (None, None));
        assert!(config.enabled && !config.web_ui);
        assert_eq!(listen_addr(&config).unwrap(), "127.0.0.1:7420".parse().unwrap());
        assert_eq!(listen_addr(&EmbeddedServerConfig::new(7420, "k")).unwrap(), "0.0.0.0:7420".parse().unwrap());
    }

    #[test]
    fn the_form_refuses_what_serve_would_refuse() {
        let cases = [
            EmbeddedServerConfigPayload { port: 0, ..form() },
            EmbeddedServerConfigPayload { auth_key: "curta".into(), ..form() },
            EmbeddedServerConfigPayload { listen_host: Some("minha-maquina".into()), ..form() },
            EmbeddedServerConfigPayload { tls_cert: Some("/c.pem".into()), ..form() },
            EmbeddedServerConfigPayload { tls_cert: Some("/c.pem".into()), tls_key: Some("/k.pem".into()), tailscale_cert: true, ..form() },
            EmbeddedServerConfigPayload { tls_host: Some("hub.example.com".into()), ..form() },
        ];
        for case in cases {
            assert!(case.into_config(false).is_err());
        }
    }

    #[test]
    fn stopped_status_serializes_as_camel_case() {
        assert_eq!(
            serde_json::to_string(&EmbeddedServerStatusPayload::stopped()).unwrap(),
            r#"{"running":false,"boundAddr":null,"serverName":null,"secure":false,"secureUrl":null,"webUrl":null}"#
        );
    }
}
