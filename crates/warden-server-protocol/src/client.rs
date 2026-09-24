use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rustls::ClientConfig;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use warden_core::tool::ToolSpec;

use crate::protocol::{ClientMessage, ServerMessage};

/// A connection to a `warden-server`, past the Hello/HelloAck handshake.
///
/// This is the reusable half of the Fase 9.2 protocol: Fase 7.2 (desktop/mobile as a client)
/// is expected to depend on `warden-server` just for this type, without pulling in anything
/// server-side.
pub struct ServerConnection {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl ServerConnection {
    /// Connects to `url` (e.g. `ws://host:7420`) and performs the Hello/HelloAck handshake.
    /// Fails if the connection can't be established, the auth key is rejected, or the server's
    /// first reply isn't `HelloAck`.
    pub async fn connect(
        url: &str,
        device_id: &str,
        device_name: &str,
        auth_key: &str,
    ) -> anyhow::Result<Self> {
        Self::connect_with_tools(url, device_id, device_name, auth_key, Vec::new()).await
    }

    /// Same as `connect`, but also advertises `tools` this client can run locally (Fase 7.4) —
    /// `Server` registers a `RemoteTool` proxy for each one on this connection's `Orchestrator`.
    pub async fn connect_with_tools(
        url: &str,
        device_id: &str,
        device_name: &str,
        auth_key: &str,
        tools: Vec<ToolSpec>,
    ) -> anyhow::Result<Self> {
        Ok(Self::handshake(url, device_id, device_name, auth_key, None, tools).await?.0)
    }

    /// Same as `connect_with_tools`, but presents the device token `tokens` holds for this
    /// `url`/`device_id` (P36) and stores whatever new one the hub issues — what a long-lived
    /// client (`warden-node`, `RemoteNodeProvider`) uses so it keeps its pairing (and `Approved`
    /// status) across reconnects and pairing-key rotations.
    pub async fn connect_with_token_store(
        url: &str,
        device_id: &str,
        device_name: &str,
        auth_key: &str,
        tools: Vec<ToolSpec>,
        tokens: &DeviceTokenStore,
    ) -> anyhow::Result<Self> {
        let stored = tokens.get(url, device_id)?;
        let (conn, issued) = Self::handshake(url, device_id, device_name, auth_key, stored, tools).await?;
        if let Some(token) = issued {
            tokens.set(url, device_id, &token)?;
        }
        Ok(conn)
    }

    /// Hello/HelloAck with an optional device token — returns the connection plus the token the
    /// hub issued, if it issued one. A `wss://` URL is verified against the public web roots
    /// (`tls::default_client_config`).
    pub async fn handshake(
        url: &str,
        device_id: &str,
        device_name: &str,
        auth_key: &str,
        device_token: Option<String>,
        tools: Vec<ToolSpec>,
    ) -> anyhow::Result<(Self, Option<String>)> {
        Self::handshake_with_tls(url, device_id, device_name, auth_key, device_token, tools, crate::tls::default_client_config()).await
    }

    /// Same as `handshake`, verifying a `wss://` hub with `tls` instead of the default roots —
    /// what tests use to trust a throwaway CA. Ignored for `ws://`.
    pub async fn handshake_with_tls(
        url: &str,
        device_id: &str,
        device_name: &str,
        auth_key: &str,
        device_token: Option<String>,
        tools: Vec<ToolSpec>,
        tls: Arc<ClientConfig>,
    ) -> anyhow::Result<(Self, Option<String>)> {
        let (ws, _response) = tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(Connector::Rustls(tls)))
            .await
            .map_err(|err| match err {
                // P36: what a TLS-only hub answers a plain `ws://` Hello with — say what to do
                // instead of surfacing a bare "HTTP error: 426".
                WsError::Http(response) if response.status() == StatusCode::UPGRADE_REQUIRED => {
                    anyhow::anyhow!("this hub only accepts encrypted connections — connect with wss:// to a name its certificate covers")
                }
                other => other.into(),
            })?;
        let mut conn = Self { ws };

        conn.send(&ClientMessage::Hello {
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            auth_key: auth_key.to_string(),
            device_token,
            tools,
        })
        .await?;

        match conn.recv().await? {
            Some(ServerMessage::HelloAck { device_token, .. }) => Ok((conn, device_token)),
            Some(ServerMessage::AuthError { reason }) => {
                anyhow::bail!("authentication rejected: {reason}")
            }
            Some(other) => anyhow::bail!("expected HelloAck, got {other:?}"),
            None => anyhow::bail!("server closed the connection before replying to Hello"),
        }
    }

    pub async fn send(&mut self, msg: &ClientMessage) -> anyhow::Result<()> {
        let json = serde_json::to_string(msg)?;
        self.ws.send(Message::Text(json.into())).await?;
        Ok(())
    }

    /// Reads the next application-level message, skipping WebSocket control frames.
    /// Returns `Ok(None)` once the socket has closed cleanly.
    pub async fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        loop {
            let Some(frame) = self.ws.next().await else {
                return Ok(None);
            };
            match frame? {
                Message::Text(text) => return Ok(Some(serde_json::from_str(&text)?)),
                Message::Close(_) => return Ok(None),
                _ => continue,
            }
        }
    }

    pub async fn ping(&mut self, nonce: u64) -> anyhow::Result<()> {
        self.send(&ClientMessage::Ping { nonce }).await
    }
}

/// Client-side device tokens (P36), one JSON file mapping `"<hub url>|<device_id>"` to the token
/// that hub issued. Same stateless read-mutate-write posture as the hub's `PairingStore` — written
/// at most once per pairing, so there's nothing worth caching.
pub struct DeviceTokenStore {
    path: PathBuf,
}

impl DeviceTokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn key(url: &str, device_id: &str) -> String {
        format!("{url}|{device_id}")
    }

    fn load(&self) -> anyhow::Result<HashMap<String, String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(serde_json::from_str(&contents)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn get(&self, url: &str, device_id: &str) -> anyhow::Result<Option<String>> {
        Ok(self.load()?.remove(&Self::key(url, device_id)))
    }

    pub fn set(&self, url: &str, device_id: &str, token: &str) -> anyhow::Result<()> {
        let mut tokens = self.load()?;
        tokens.insert(Self::key(url, device_id), token.to_string());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&tokens)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceTokenStore;

    #[test]
    fn tokens_are_kept_per_hub_and_device_and_survive_a_fresh_store() {
        let path = std::env::temp_dir().join(format!(
            "warden-device-tokens-test-{}.json",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let store = DeviceTokenStore::new(&path);
        assert_eq!(store.get("ws://a:7420", "dev-1").unwrap(), None);

        store.set("ws://a:7420", "dev-1", "tok-a").unwrap();
        store.set("ws://b:7420", "dev-1", "tok-b").unwrap();
        store.set("ws://a:7420", "dev-1", "tok-a2").unwrap();

        let reloaded = DeviceTokenStore::new(&path);
        assert_eq!(reloaded.get("ws://a:7420", "dev-1").unwrap().as_deref(), Some("tok-a2"));
        assert_eq!(reloaded.get("ws://b:7420", "dev-1").unwrap().as_deref(), Some("tok-b"));
        assert_eq!(reloaded.get("ws://a:7420", "dev-2").unwrap(), None);
    }
}
