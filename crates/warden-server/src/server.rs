use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message};
use tokio_tungstenite::WebSocketStream;

use crate::protocol::{ClientMessage, ServerMessage};

type WsSink = SplitSink<WebSocketStream<TcpStream>, Message>;

/// The server side of the Fase 9 client↔server WebSocket protocol.
///
/// Deliberately has no `Orchestrator`/tool-dispatch surface — this only proves the wire
/// protocol (handshake + heartbeat) works end to end. Routing tool calls to a specific
/// connected client is Fase 9.4/9.5, not this.
pub struct Server {
    listener: TcpListener,
    auth_key: Arc<str>,
}

impl Server {
    pub async fn bind(addr: SocketAddr, auth_key: impl Into<Arc<str>>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            auth_key: auth_key.into(),
        })
    }

    /// Actual bound address — useful when `bind` was called with port 0 (OS-assigned), e.g. in tests.
    pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    /// Accepts connections forever, one task per connection. Only returns on a listener-level
    /// error (a single connection's failure never brings the server down).
    pub async fn serve(self) -> anyhow::Result<()> {
        loop {
            let (stream, peer) = self.listener.accept().await?;
            let auth_key = self.auth_key.clone();
            tokio::spawn(async move {
                if let Err(err) = handle_connection(stream, peer, auth_key).await {
                    eprintln!("warden-server: connection from {peer} ended with error: {err:#}");
                }
            });
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    auth_key: Arc<str>,
) -> anyhow::Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut stream) = ws.split();

    let Some(first) = stream.next().await else {
        return Ok(());
    };
    let first = first?;
    let Message::Text(text) = first else {
        eprintln!("warden-server: {peer} sent a non-text first frame, closing");
        return Ok(());
    };
    let hello = match serde_json::from_str::<ClientMessage>(&text) {
        Ok(ClientMessage::Hello {
            device_id,
            device_name,
            auth_key: provided,
        }) => (device_id, device_name, provided),
        Ok(_) => {
            eprintln!("warden-server: {peer} didn't send Hello first, closing");
            return Ok(());
        }
        Err(err) => {
            eprintln!("warden-server: {peer} sent an unparseable first frame: {err}");
            return Ok(());
        }
    };
    let (device_id, device_name, provided_key) = hello;

    if provided_key.as_str() != auth_key.as_ref() {
        send(&mut sink, &ServerMessage::AuthError {
            reason: "invalid auth key".into(),
        })
        .await?;
        sink.send(Message::Close(Some(CloseFrame {
            code: CloseCode::Policy,
            reason: "invalid auth key".into(),
        })))
        .await?;
        return Ok(());
    }

    eprintln!("warden-server: {device_name} ({device_id}) connected from {peer}");
    send(&mut sink, &ServerMessage::HelloAck {
        server_name: "warden-server".into(),
    })
    .await?;

    while let Some(frame) = stream.next().await {
        match frame? {
            Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::Ping { nonce }) => {
                    send(&mut sink, &ServerMessage::Pong { nonce }).await?;
                }
                Ok(ClientMessage::Goodbye { reason }) => {
                    eprintln!("warden-server: {device_id} said goodbye ({reason:?})");
                    break;
                }
                Ok(ClientMessage::Hello { .. }) => {
                    eprintln!("warden-server: {device_id} sent a second Hello, ignoring");
                }
                Err(err) => {
                    eprintln!("warden-server: {device_id} sent an unparseable message: {err}");
                    break;
                }
            },
            Message::Close(_) => break,
            _ => {}
        }
    }

    Ok(())
}

async fn send(sink: &mut WsSink, msg: &ServerMessage) -> anyhow::Result<()> {
    let json = serde_json::to_string(msg)?;
    sink.send(Message::Text(json.into())).await?;
    Ok(())
}
