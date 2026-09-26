// Shared across multiple test binaries (`handshake.rs`, `chat.rs`); each one only exercises a
// subset of these constructors, so the ones unused by any given binary would otherwise warn.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::{Tool, ToolSpec};
use warden_server::Server;

/// Per-host timeout for `discover_hubs_on` in tests — far above the real 800 ms, since a debug
/// build on a busy machine can take seconds to answer a local probe (P81).
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// A `ModelProvider` test double — no real API key needed, same helper (`response_stream`)
/// `warden-core`'s own unit tests use to turn a canned `Response` into a `ChatStream`.
pub struct MockProvider {
    pub reply: String,
    pub delay: Duration,
    pub fail: bool,
    /// When set, the *first* `chat_stream` call returns a tool call instead of `reply` — every
    /// call after that returns `reply` with no tool calls, same two-round shape
    /// `Orchestrator::handle_turn`'s own tool-calling loop expects (see `warden-core`'s
    /// `orchestrator::tests::MockModel`).
    pub tool_call: Option<(String, Value)>,
    calls: AtomicUsize,
}

impl MockProvider {
    pub fn replying(reply: impl Into<String>) -> Self {
        Self { reply: reply.into(), delay: Duration::ZERO, fail: false, tool_call: None, calls: AtomicUsize::new(0) }
    }

    pub fn failing() -> Self {
        Self { reply: String::new(), delay: Duration::ZERO, fail: true, tool_call: None, calls: AtomicUsize::new(0) }
    }

    pub fn replying_after(reply: impl Into<String>, delay: Duration) -> Self {
        Self { reply: reply.into(), delay, fail: false, tool_call: None, calls: AtomicUsize::new(0) }
    }

    /// First `chat_stream` call requests `tool(arguments)`; the *second* call's reply is derived
    /// straight from the real `Role::Tool` result message the orchestrator fed back — not a
    /// canned string — so a test asserting on the final `ChatResponse.content` is actually proving
    /// the client's real tool result made it all the way back through the model, not just that a
    /// request/response pair of frames was exchanged.
    pub fn calling_tool_then_replying(tool: impl Into<String>, arguments: Value) -> Self {
        Self { reply: String::new(), delay: Duration::ZERO, fail: false, tool_call: Some((tool.into(), arguments)), calls: AtomicUsize::new(0) }
    }
}

#[async_trait]
impl ModelProvider for MockProvider {
    async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if self.fail {
            anyhow::bail!("mock provider failure");
        }

        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((name, arguments)) = &self.tool_call {
            if call == 0 {
                return Ok(response_stream(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".to_string(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                        thought_signature: None,
                    }],
                    usage: None,
                }));
            }
            let tool_result = messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Tool)
                .map(|m| m.content.clone())
                .unwrap_or_default();
            return Ok(response_stream(Response { content: format!("tool said: {tool_result}"), tool_calls: Vec::new(), usage: None }));
        }
        Ok(response_stream(Response { content: self.reply.clone(), tool_calls: Vec::new(), usage: None }))
    }
}

/// Spins up a real `Server` (bound to an OS-assigned localhost port) with `provider` as its
/// model — used by every integration test in this crate so none of them need a real API key.
pub async fn spin_up_server(provider: MockProvider) -> std::net::SocketAddr {
    spin_up_server_with_devices_path(provider).await.0
}

/// Same as `spin_up_server`, but also hands back the path of this server's (fresh, empty)
/// pairing registry — needed by any test that exercises `CallDeviceTool` (Fase 9.3 requires the
/// caller and target to be `Approved` there first, see `warden_server::PairingStore`).
pub async fn spin_up_server_with_devices_path(provider: MockProvider) -> (std::net::SocketAddr, std::path::PathBuf) {
    spin_up_server_with_revocation_check(provider, warden_server::server::DEFAULT_REVOCATION_CHECK_INTERVAL).await
}

/// Same as `spin_up_server_with_devices_path`, with the open-connection revocation check (P36)
/// running every `interval` — short in tests so a revocation shows up without waiting seconds.
pub async fn spin_up_server_with_revocation_check(provider: MockProvider, interval: Duration) -> (std::net::SocketAddr, std::path::PathBuf) {
    let temp_dir = std::env::temp_dir().join(format!(
        "warden-server-test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let vault = Arc::new(Vault::new(temp_dir.join("vault")));
    let orchestrator = Orchestrator::new(Arc::new(provider), vault);
    let devices_path = temp_dir.join("devices.json");

    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), temp_dir.join("conversations"), devices_path.clone())
        .await
        .unwrap()
        .with_revocation_check_interval(interval);
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    (addr, devices_path)
}

/// Same as `spin_up_server`, but the shared `Orchestrator` already has `base_tool` registered —
/// needed by any test that exercises P42 (a connecting client's `Hello.tools` colliding with a
/// tool the server already has, not with another client's — each connection gets its own clone of
/// the shared orchestrator, so two different clients never collide with each other).
pub async fn spin_up_server_with_base_tool(provider: MockProvider, base_tool: Arc<dyn Tool>) -> std::net::SocketAddr {
    let temp_dir = std::env::temp_dir().join(format!(
        "warden-server-test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let vault = Arc::new(Vault::new(temp_dir.join("vault")));
    let mut orchestrator = Orchestrator::new(Arc::new(provider), vault);
    orchestrator.register_tool(base_tool);

    let server = Server::bind(
        "127.0.0.1:0".parse().unwrap(),
        "test-key",
        "Test Hub",
        Arc::new(orchestrator),
        temp_dir.join("conversations"),
        temp_dir.join("devices.json"),
    )
    .await
    .unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    addr
}

/// Same as `spin_up_server`, but TLS-only (P36) with `tls`. Returns the bound address.
pub async fn spin_up_tls_server(provider: MockProvider, tls: warden_server::HubTls) -> std::net::SocketAddr {
    let temp_dir = std::env::temp_dir().join(format!(
        "warden-server-test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let vault = Arc::new(Vault::new(temp_dir.join("vault")));
    let orchestrator = Orchestrator::new(Arc::new(provider), vault);

    let server = Server::bind(
        "127.0.0.1:0".parse().unwrap(),
        "test-key",
        "Test Hub",
        Arc::new(orchestrator),
        temp_dir.join("conversations"),
        temp_dir.join("devices.json"),
    )
    .await
    .unwrap()
    .with_tls(tls);
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    addr
}

/// Same as `spin_up_server`, but runs `serve_until` (not `serve`) so the caller can stop it — the
/// returned `oneshot::Sender` is what a graceful-shutdown test fires.
pub async fn spin_up_server_with_shutdown(provider: MockProvider) -> (std::net::SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let temp_dir = std::env::temp_dir().join(format!(
        "warden-server-test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let vault = Arc::new(Vault::new(temp_dir.join("vault")));
    let orchestrator = Orchestrator::new(Arc::new(provider), vault);

    let server = Server::bind(
        "127.0.0.1:0".parse().unwrap(),
        "test-key",
        "Test Hub",
        Arc::new(orchestrator),
        temp_dir.join("conversations"),
        temp_dir.join("devices.json"),
    )
    .await
    .unwrap();
    let addr = server.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(server.serve_until(async {
        let _ = shutdown_rx.await;
    }));
    (addr, shutdown_tx)
}

/// A fresh, empty client-side `DeviceTokenStore` (P36) — for the clients that keep their token
/// (`RemoteNodeProvider`, `vault_node::connect`).
pub fn temp_token_store() -> warden_server::DeviceTokenStore {
    warden_server::DeviceTokenStore::new(std::env::temp_dir().join(format!(
        "warden-server-test-tokens-{}.json",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    )))
}

/// Connects `device_id` (registering it as `Pending` in the pairing registry via a real `Hello`),
/// then immediately approves it — the common setup every `CallDeviceTool` test needs for a device
/// to be allowed to call or be called. Returns the now-approved connection.
pub async fn connect_and_approve(addr: std::net::SocketAddr, devices_path: &std::path::Path, device_id: &str, device_name: &str) -> warden_server::ServerConnection {
    let conn = warden_server::ServerConnection::connect(&format!("ws://{addr}"), device_id, device_name, "test-key").await.unwrap();
    warden_server::PairingStore::new(devices_path.to_path_buf()).approve(device_id).unwrap();
    conn
}

/// A small in-memory web UI (P78): `index.html` plus one hashed asset, as `web/dist` would have.
pub fn test_web_ui() -> Arc<dyn warden_server::WebAssets> {
    let mut files = std::collections::HashMap::new();
    files.insert("index.html".to_string(), b"<!doctype html><title>Warden test UI</title>".to_vec());
    files.insert("assets/app-1a2b.js".to_string(), b"console.log('warden')".to_vec());
    Arc::new(warden_server::StaticWebUi(files))
}

/// Same as `spin_up_server` (or `spin_up_tls_server`, given `tls`), also serving `web_ui` (P78).
pub async fn spin_up_server_with_web_ui(provider: MockProvider, web_ui: Arc<dyn warden_server::WebAssets>, tls: Option<warden_server::HubTls>) -> std::net::SocketAddr {
    let temp_dir = std::env::temp_dir().join(format!(
        "warden-server-test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let vault = Arc::new(Vault::new(temp_dir.join("vault")));
    let orchestrator = Orchestrator::new(Arc::new(provider), vault);

    let mut server = Server::bind(
        "127.0.0.1:0".parse().unwrap(),
        "test-key",
        "Test Hub",
        Arc::new(orchestrator),
        temp_dir.join("conversations"),
        temp_dir.join("devices.json"),
    )
    .await
    .unwrap()
    .with_web_ui(web_ui);
    if let Some(tls) = tls {
        server = server.with_tls(tls);
    }
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    addr
}

/// Sends `request` (a raw HTTP/1.1 request) over `stream` and reads until the hub closes — the web
/// UI answers one request per connection.
pub async fn raw_http<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(mut stream: S, request: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    String::from_utf8_lossy(&response).into_owned()
}

/// `raw_http` over a plain TCP connection to `addr`.
pub async fn http_get(addr: std::net::SocketAddr, method: &str, path: &str) -> String {
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    raw_http(stream, &format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n")).await
}
