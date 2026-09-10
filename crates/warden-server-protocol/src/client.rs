use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
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
        let (ws, _response) = tokio_tungstenite::connect_async(url).await?;
        let mut conn = Self { ws };

        conn.send(&ClientMessage::Hello {
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            auth_key: auth_key.to_string(),
            tools,
        })
        .await?;

        match conn.recv().await? {
            Some(ServerMessage::HelloAck { .. }) => Ok(conn),
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
