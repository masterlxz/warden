use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rustls::ClientConfig;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use warden_core::tool::ToolSpec;

use crate::protocol::{ClientMessage, NodeOfferDto, ServerMessage, UserInfoDto};

/// The hub turned this client away (`AuthError`): a wrong key, or a token that was revoked. Typed so
/// a caller that reconnects on its own (a node, P97) can tell it apart from a hub that's just away.
#[derive(Debug)]
pub struct AuthRejected {
    pub reason: String,
}

impl std::fmt::Display for AuthRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "authentication rejected: {}", self.reason)
    }
}

impl std::error::Error for AuthRejected {}

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
        Self::handshake_full(url, device_id, device_name, auth_key, device_token, tools, None, tls).await
    }

    /// The general form: also announces this client as a node (P93) when `node` is set.
    #[allow(clippy::too_many_arguments)]
    pub async fn handshake_full(
        url: &str,
        device_id: &str,
        device_name: &str,
        auth_key: &str,
        device_token: Option<String>,
        tools: Vec<ToolSpec>,
        node: Option<NodeOfferDto>,
        tls: Arc<ClientConfig>,
    ) -> anyhow::Result<(Self, Option<String>)> {
        let hello = ClientMessage::Hello {
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            auth_key: auth_key.to_string(),
            device_token,
            tools,
            node,
            username: None,
            password: None,
            recovery_codes: false,
        };
        let (conn, token, _) = Self::hello(url, hello, tls).await?;
        Ok((conn, token))
    }

    /// P84: pairs as a member with their username and password (or reconnects with the token a
    /// previous one issued), and says who the hub took this device to be.
    pub async fn handshake_as_member(
        url: &str,
        device_id: &str,
        device_name: &str,
        username: &str,
        password: &str,
        device_token: Option<String>,
        tls: Arc<ClientConfig>,
    ) -> anyhow::Result<(Self, Option<String>, Option<UserInfoDto>)> {
        let hello = ClientMessage::Hello {
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            auth_key: String::new(),
            device_token,
            tools: Vec::new(),
            node: None,
            username: Some(username.to_string()),
            password: Some(password.to_string()),
            // This client hands the recovery code back to its caller, who has to show it.
            recovery_codes: true,
        };
        Self::hello(url, hello, tls).await
    }

    async fn hello(url: &str, hello: ClientMessage, tls: Arc<ClientConfig>) -> anyhow::Result<(Self, Option<String>, Option<UserInfoDto>)> {
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
        conn.send(&hello).await?;

        match conn.recv().await? {
            Some(ServerMessage::HelloAck { device_token, user, .. }) => Ok((conn, device_token, user)),
            Some(ServerMessage::AuthError { reason }) => Err(AuthRejected { reason }.into()),
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
