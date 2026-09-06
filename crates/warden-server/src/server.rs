use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message};
use tokio_tungstenite::WebSocketStream;
use warden_core::orchestrator::Orchestrator;

use crate::protocol::{ClientMessage, ServerMessage};

type WsSink = SplitSink<WebSocketStream<TcpStream>, Message>;

/// The server side of the Fase 9 client↔server WebSocket protocol.
///
/// As of Fase 7.3, hosts a real `Orchestrator` and answers `Chat` messages with it — routing a
/// tool call to a *specific* connected client (Fase 9.4/9.5) is still out of scope, but "the
/// server has a model to talk to" no longer is.
pub struct Server {
    listener: TcpListener,
    auth_key: Arc<str>,
    orchestrator: Arc<Orchestrator>,
    conversations_dir: Arc<PathBuf>,
}

impl Server {
    pub async fn bind(
        addr: SocketAddr,
        auth_key: impl Into<Arc<str>>,
        orchestrator: Arc<Orchestrator>,
        conversations_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            auth_key: auth_key.into(),
            orchestrator,
            conversations_dir: Arc::new(conversations_dir),
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
            let orchestrator = self.orchestrator.clone();
            let conversations_dir = self.conversations_dir.clone();
            tokio::spawn(async move {
                if let Err(err) = handle_connection(stream, peer, auth_key, orchestrator, conversations_dir).await {
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
    orchestrator: Arc<Orchestrator>,
    conversations_dir: Arc<PathBuf>,
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

    // From here on, outgoing messages go through a channel to a dedicated writer task instead of
    // straight to `sink` — a `Chat` message can take a long time (real model latency, 10s-70s+
    // observed with Gemini), and this reader loop must keep servicing `Ping` frames the whole
    // time or the client's heartbeat wrongly concludes the connection is dead (it treats an
    // unanswered ping past the next ~30s interval as a drop). Handling `Chat` in its own spawned
    // task, writing back through this channel, keeps the reader loop free to answer `Ping`
    // immediately regardless of how long any in-flight `Chat` call takes.
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();
    let writer_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if send(&mut sink, &msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(frame) = stream.next().await {
        match frame? {
            Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::Ping { nonce }) => {
                    let _ = tx.send(ServerMessage::Pong { nonce });
                }
                Ok(ClientMessage::Chat { message }) => {
                    let orchestrator = orchestrator.clone();
                    let conversations_dir = conversations_dir.clone();
                    let device_id = device_id.clone();
                    let reply_tx = tx.clone();
                    tokio::spawn(async move {
                        let reply = match warden_bootstrap::handle_turn(
                            &orchestrator,
                            &conversations_dir,
                            &device_id,
                            &message,
                            &message,
                        )
                        .await
                        {
                            Ok(outcome) => ServerMessage::ChatResponse { content: outcome.content, usage: outcome.usage },
                            Err(err) => ServerMessage::ChatError { message: format!("{err:#}") },
                        };
                        let _ = reply_tx.send(reply);
                    });
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

    drop(tx);
    writer_task.await.ok();

    Ok(())
}

async fn send(sink: &mut WsSink, msg: &ServerMessage) -> anyhow::Result<()> {
    let json = serde_json::to_string(msg)?;
    sink.send(Message::Text(json.into())).await?;
    Ok(())
}
