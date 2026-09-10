//! `warden_core::storage::StorageProvider` face for a vault living on a *different* connected
//! device (P61's `RemoteNodeProvider`, v2) — the caller side of Fase 9.3/9.4's cross-device
//! routing (`server.rs`'s device registry + `ClientMessage::CallDeviceTool`). The target side (a
//! process that actually connects as a client and serves `vault_read`/`vault_write`/`vault_list`/
//! `vault_delete` against its own local `Vault`) doesn't exist yet — this module only proves the
//! calling half works, tested against a scripted fake target (see `tests/remote_node_provider.rs`).
//! See `project/PENDING.md` P61 for what's still open.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use warden_core::storage::StorageProvider;

use crate::client::ServerConnection;
use crate::protocol::{ClientMessage, ServerMessage};

/// Same reasoning as `remote_tool::DEFAULT_TIMEOUT` — a vault read/write/list/delete on another
/// machine taking this long means that machine (or the connection to it) is gone, not just slow.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

type PendingCalls = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

fn resolve(pending: &PendingCalls, call_id: u64, result: Result<Value, String>) {
    if let Some(tx) = pending.lock().unwrap().remove(&call_id) {
        let _ = tx.send(result);
    }
}

/// One background task owning a `ServerConnection` to a `warden-server`, plus the call-id
/// allocator/pending map `call()` needs to correlate a `CallDeviceTool` with its eventual
/// `DeviceToolResult`/`DeviceToolError`. Not re-exported — `RemoteNodeProvider` is the public face.
///
/// A single task both sends and receives (`tokio::select!`), unlike `server.rs`'s split
/// sink/stream — there's no server-initiated message here that this client must stay responsive
/// to while a call is in flight (the protocol's only heartbeat is client-sent `Ping`/server-replied
/// `Pong`, never the reverse), so one task alternating between "send the next queued outgoing
/// message" and "handle the next incoming frame" is sufficient.
struct RemoteNodeClient {
    tx: mpsc::UnboundedSender<ClientMessage>,
    pending: PendingCalls,
    next_call_id: Arc<AtomicU64>,
    target_device_id: String,
}

impl RemoteNodeClient {
    async fn connect(url: &str, device_id: &str, device_name: &str, auth_key: &str, target_device_id: String) -> anyhow::Result<Self> {
        let conn = ServerConnection::connect(url, device_id, device_name, auth_key).await?;
        let (tx, rx) = mpsc::unbounded_channel::<ClientMessage>();
        let pending: PendingCalls = Arc::new(Mutex::new(HashMap::new()));

        tokio::spawn(run_connection(conn, rx, pending.clone()));

        Ok(Self { tx, pending, next_call_id: Arc::new(AtomicU64::new(0)), target_device_id })
    }

    /// Sends `CallDeviceTool` for `tool(arguments)` and waits (up to `timeout`) for the matching
    /// `DeviceToolResult`/`DeviceToolError` — same alloc/send/await/timeout shape as
    /// `remote_tool::RemoteToolChannel::call`, just targeting a different device through the
    /// server instead of asking this connection's own peer to run something.
    async fn call(&self, tool: String, arguments: Value, timeout: Duration) -> anyhow::Result<Value> {
        let call_id = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        let (result_tx, result_rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(call_id, result_tx);

        let request = ClientMessage::CallDeviceTool {
            call_id,
            target_device_id: self.target_device_id.clone(),
            tool: tool.clone(),
            arguments,
        };
        if self.tx.send(request).is_err() {
            self.pending.lock().unwrap().remove(&call_id);
            anyhow::bail!("'{tool}' failed: connection closed before the request could be sent");
        }

        match tokio::time::timeout(timeout, result_rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => anyhow::bail!("remote node '{tool}' failed: {message}"),
            Ok(Err(_canceled)) => anyhow::bail!("'{tool}' failed: connection closed while waiting for a reply"),
            Err(_elapsed) => {
                self.pending.lock().unwrap().remove(&call_id);
                anyhow::bail!("remote node '{tool}' timed out after {timeout:?}")
            }
        }
    }
}

/// Drives one `RemoteNodeClient`'s connection until it closes (server drop, network error, or the
/// client itself being dropped, which closes `rx`) — no automatic reconnect: a call issued after
/// this returns simply fails with "connection closed", same posture `RemoteToolChannel::call`
/// already takes server-side. Reconnection is a real gap for production use, deliberately left for
/// whenever this actually needs to survive a flaky link.
async fn run_connection(mut conn: ServerConnection, mut rx: mpsc::UnboundedReceiver<ClientMessage>, pending: PendingCalls) {
    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                match outgoing {
                    Some(msg) => {
                        if conn.send(&msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            incoming = conn.recv() => {
                match incoming {
                    Ok(Some(ServerMessage::DeviceToolResult { call_id, result })) => resolve(&pending, call_id, Ok(result)),
                    Ok(Some(ServerMessage::DeviceToolError { call_id, message })) => resolve(&pending, call_id, Err(message)),
                    Ok(Some(_other)) => {} // not meaningful to this client (e.g. a stray Pong)
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }
}

/// A `StorageProvider` backed by another connected device's vault, reached through a
/// `warden-server` this process is also connected to (P61's `RemoteNodeProvider`). The 4 tool
/// names/wire shapes below are the contract a future node-agent (the target side, not built yet)
/// must implement exactly.
pub struct RemoteNodeProvider {
    client: RemoteNodeClient,
}

impl RemoteNodeProvider {
    /// Connects to the `warden-server` at `url` as `device_id`/`device_name` (auth'd with
    /// `auth_key`), ready to route calls to `target_device_id` — the other connected device whose
    /// vault this provider fronts.
    pub async fn connect(
        url: &str,
        device_id: &str,
        device_name: &str,
        auth_key: &str,
        target_device_id: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let client = RemoteNodeClient::connect(url, device_id, device_name, auth_key, target_device_id.into()).await?;
        Ok(Self { client })
    }
}

#[async_trait]
impl StorageProvider for RemoteNodeProvider {
    async fn read(&self, relative_path: &str) -> anyhow::Result<Vec<u8>> {
        let result = self.client.call("vault_read".to_string(), serde_json::json!({"path": relative_path}), DEFAULT_TIMEOUT).await?;
        let content_b64 = result
            .get("content_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("vault_read reply for '{relative_path}' is missing 'content_base64': {result}"))?;
        base64::engine::general_purpose::STANDARD
            .decode(content_b64)
            .map_err(|e| anyhow::anyhow!("vault_read reply for '{relative_path}' has invalid base64: {e}"))
    }

    async fn write(&self, relative_path: &str, content: &[u8]) -> anyhow::Result<()> {
        let content_base64 = base64::engine::general_purpose::STANDARD.encode(content);
        self.client
            .call("vault_write".to_string(), serde_json::json!({"path": relative_path, "content_base64": content_base64}), DEFAULT_TIMEOUT)
            .await?;
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<String>> {
        let result = self.client.call("vault_list".to_string(), serde_json::json!({}), DEFAULT_TIMEOUT).await?;
        let paths = result.get("paths").and_then(Value::as_array).ok_or_else(|| anyhow::anyhow!("vault_list reply is missing 'paths': {result}"))?;
        paths
            .iter()
            .map(|p| p.as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("vault_list reply has a non-string path: {p}")))
            .collect()
    }

    async fn delete(&self, relative_path: &str) -> anyhow::Result<()> {
        self.client.call("vault_delete".to_string(), serde_json::json!({"path": relative_path}), DEFAULT_TIMEOUT).await?;
        Ok(())
    }
}
