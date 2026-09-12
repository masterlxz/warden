use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message};
use tokio_tungstenite::WebSocketStream;
use warden_core::orchestrator::Orchestrator;

use crate::device_registry::{PairingStatus, PairingStore};
use crate::remote_tool::{RemoteTool, RemoteToolChannel, DEFAULT_TIMEOUT as REMOTE_TOOL_TIMEOUT};
use warden_server_protocol::{ClientMessage, ServerMessage};

type WsSink = SplitSink<WebSocketStream<TcpStream>, Message>;

/// Every currently-connected device's `RemoteToolChannel`, keyed by `device_id` — populated on a
/// successful `Hello` (Fase 9.3), regardless of whether that device advertised any `Hello.tools`
/// (a device is a valid routing *target* just by being connected; advertising tools only matters
/// for Fase 7.4's "the model chatting with this same device can invoke them"). Consulted by
/// `ClientMessage::CallDeviceTool` (Fase 9.4) to reach a specific *other* connection. Known
/// limitation, not handled here: a device that reconnects while its previous connection's cleanup
/// is still unwinding could have the new registration clobbered by the old one's removal — no
/// reconnect scenario exists yet to make that a real problem.
type DeviceRegistry = Arc<Mutex<HashMap<String, RemoteToolChannel>>>;

/// The server side of the Fase 9 client↔server WebSocket protocol.
///
/// As of Fase 7.3, hosts a real `Orchestrator` and answers `Chat` messages with it. As of this
/// session (Fase 9.3/9.4), also keeps a registry of every connected device and routes
/// `CallDeviceTool` from one connection to a specific other one — the client-side piece that
/// would actually *use* this routing (e.g. a `StorageProvider` backed by another machine's vault,
/// P61's `RemoteNodeProvider`) doesn't exist yet; this is the server-side foundation for it.
pub struct Server {
    listener: TcpListener,
    auth_key: Arc<str>,
    orchestrator: Arc<Orchestrator>,
    conversations_dir: Arc<PathBuf>,
    devices: DeviceRegistry,
    devices_path: Arc<PathBuf>,
}

impl Server {
    pub async fn bind(
        addr: SocketAddr,
        auth_key: impl Into<Arc<str>>,
        orchestrator: Arc<Orchestrator>,
        conversations_dir: PathBuf,
        devices_path: PathBuf,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            auth_key: auth_key.into(),
            orchestrator,
            conversations_dir: Arc::new(conversations_dir),
            devices: Arc::new(Mutex::new(HashMap::new())),
            devices_path: Arc::new(devices_path),
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
            let devices = self.devices.clone();
            let devices_path = self.devices_path.clone();
            tokio::spawn(async move {
                if let Err(err) = handle_connection(stream, peer, auth_key, orchestrator, conversations_dir, devices, devices_path).await {
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
    devices: DeviceRegistry,
    devices_path: Arc<PathBuf>,
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
            tools,
        }) => (device_id, device_name, provided, tools),
        Ok(_) => {
            eprintln!("warden-server: {peer} didn't send Hello first, closing");
            return Ok(());
        }
        Err(err) => {
            eprintln!("warden-server: {peer} sent an unparseable first frame: {err}");
            return Ok(());
        }
    };
    let (device_id, device_name, provided_key, tools) = hello;

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

    // Fase 9.3: record this device in the persistent pairing registry — silent from the client's
    // perspective (still gets a normal `HelloAck` below even as `Pending`), only surfaced via
    // `warden-server devices list`/`approve`. Only `CallDeviceTool` actually checks the status.
    if let Err(err) = PairingStore::new(devices_path.as_ref().clone()).record_seen(&device_id, &device_name) {
        eprintln!("warden-server: failed to record '{device_id}' in the pairing registry: {err:#}");
    }

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

    // Fase 9.3: every connected device is a valid routing target for `CallDeviceTool`, whether or
    // not it advertised any `Hello.tools` — registered before the loop starts so a routed call
    // arriving right after this device's own Hello can never race the registration.
    let tool_channel = RemoteToolChannel::new(tx.clone());
    devices.lock().unwrap().insert(device_id.clone(), tool_channel.clone());

    // Fase 7.4: a client that advertised tools in Hello gets its own Orchestrator (cheap clone —
    // Orchestrator is Arc-backed) with a RemoteTool proxy per advertised spec, so the model can
    // invoke a capability that only exists on *this* device (mobile's file access, to start). A
    // client with nothing to advertise (tools empty) just reuses the shared, server-wide instance.
    let orchestrator: Arc<Orchestrator> = if tools.is_empty() {
        orchestrator
    } else {
        let mut per_connection = (*orchestrator).clone();
        for spec in tools {
            per_connection.register_tool(Arc::new(RemoteTool::new(spec, tool_channel.clone(), REMOTE_TOOL_TIMEOUT)));
        }
        Arc::new(per_connection)
    };

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
                Ok(ClientMessage::ToolCallResult { call_id, result }) => {
                    tool_channel.resolve(call_id, Ok(result));
                }
                Ok(ClientMessage::ToolCallError { call_id, message }) => {
                    tool_channel.resolve(call_id, Err(message));
                }
                Ok(ClientMessage::CallDeviceTool { call_id, target_device_id, tool, arguments }) => {
                    let target_channel = devices.lock().unwrap().get(&target_device_id).cloned();
                    let reply_tx = tx.clone();
                    let caller_id = device_id.clone();
                    let devices_path = devices_path.clone();
                    tokio::spawn(async move {
                        let store = PairingStore::new(devices_path.as_ref().clone());
                        let reply = match check_caller_approved(&store, &caller_id) {
                            Err(message) => ServerMessage::DeviceToolError { call_id, message },
                            Ok(()) => match target_channel {
                                // "not connected" wins over "not approved" here on purpose: a
                                // device that never Hello'd can never be approved either (`approve`
                                // requires a prior `record_seen`), so leading with "not approved"
                                // for that case would send the operator chasing something they
                                // structurally cannot do anything about.
                                None => ServerMessage::DeviceToolError {
                                    call_id,
                                    message: format!("device '{target_device_id}' is not connected"),
                                },
                                Some(channel) => match check_target_approved(&store, &target_device_id) {
                                    Err(message) => ServerMessage::DeviceToolError { call_id, message },
                                    Ok(()) => match channel.call(tool, arguments, REMOTE_TOOL_TIMEOUT).await {
                                        Ok(result) => ServerMessage::DeviceToolResult { call_id, result },
                                        Err(err) => ServerMessage::DeviceToolError { call_id, message: format!("{err:#}") },
                                    },
                                },
                            },
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

    devices.lock().unwrap().remove(&device_id);
    drop(tx);
    writer_task.await.ok();

    Ok(())
}

/// Fase 9.3 gate on `CallDeviceTool`: the calling device must be `Approved` in the persistent
/// pairing registry — being connected and knowing the shared `auth_key` is no longer enough on
/// its own to route a call *to* someone else.
fn check_caller_approved(store: &PairingStore, caller_id: &str) -> Result<(), String> {
    match store.status(caller_id).map_err(|e| format!("failed to check pairing status: {e:#}"))? {
        Some(PairingStatus::Approved) => Ok(()),
        Some(PairingStatus::Revoked) => Err(format!("your device '{caller_id}' has been revoked and can no longer route tool calls")),
        Some(PairingStatus::Pending) | None => {
            Err(format!("your device '{caller_id}' is not approved for routing yet — ask the operator to run `warden-server devices approve {caller_id}`"))
        }
    }
}

/// Same gate as `check_caller_approved`, for the target side — only called once the target is
/// known to be connected (see the "not connected wins over not approved" note at the call site).
fn check_target_approved(store: &PairingStore, target_id: &str) -> Result<(), String> {
    match store.status(target_id).map_err(|e| format!("failed to check pairing status: {e:#}"))? {
        Some(PairingStatus::Approved) => Ok(()),
        Some(PairingStatus::Revoked) => Err(format!("device '{target_id}' has been revoked and can no longer be routed to")),
        Some(PairingStatus::Pending) | None => {
            Err(format!("device '{target_id}' is not approved for routing yet — ask the operator to run `warden-server devices approve {target_id}`"))
        }
    }
}

async fn send(sink: &mut WsSink, msg: &ServerMessage) -> anyhow::Result<()> {
    let json = serde_json::to_string(msg)?;
    sink.send(Message::Text(json.into())).await?;
    Ok(())
}
