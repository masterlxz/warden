use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message};
use tokio_tungstenite::WebSocketStream;
use warden_core::orchestrator::Orchestrator;
use warden_core::skill::SkillStore;
use warden_core::spend::SpendContext;

use crate::chat_input::{handle_transcribe, title_seed, validate_attachments, Transcriber};
use crate::conversations::{device_conversations_dir, handle_conversation_request, handle_history_request, resolve_conversation_id};
use crate::device_registry::{AuthRejection, PairingStatus, PairingStore};
use crate::skills::handle_skill_request;
use crate::usage::{handle_extend_limit, handle_usage_request, spend_limit_id};
use crate::vault::handle_vault_request;
use crate::remote_tool::{RemoteTool, RemoteToolChannel, DEFAULT_TIMEOUT as REMOTE_TOOL_TIMEOUT};
use crate::tls::HubTls;
use crate::web_ui::{self, Rewind, WebAssets};
use warden_server_protocol::tls::DISCOVER_PATH;
use warden_server_protocol::{ClientMessage, ServerMessage};

/// A connection's transport — plain TCP, or TCP under TLS (P36) — past the point where it matters.
trait Transport: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> Transport for S {}

type WsSink<S> = SplitSink<WebSocketStream<S>, Message>;

/// First byte of every TLS connection (a handshake record carrying the ClientHello); a plain
/// WebSocket upgrade starts with the `G` of `GET`. Lets one port serve both (P36).
const TLS_HANDSHAKE_RECORD: u8 = 0x16;

/// How long a new connection gets to send its first byte and finish the TLS handshake.
const TLS_ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);

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
    server_name: Arc<str>,
    orchestrator: Arc<Orchestrator>,
    conversations_dir: Arc<PathBuf>,
    devices: DeviceRegistry,
    devices_path: Arc<PathBuf>,
    revocation_check_interval: Duration,
    tls: Option<HubTls>,
    web_ui: Option<Arc<dyn WebAssets>>,
    transcriber: Option<Arc<dyn Transcriber>>,
}

/// How often an open connection re-reads the pairing registry to notice it was revoked (P36).
pub const DEFAULT_REVOCATION_CHECK_INTERVAL: Duration = Duration::from_secs(5);

impl Server {
    pub async fn bind(
        addr: SocketAddr,
        auth_key: impl Into<Arc<str>>,
        server_name: impl Into<Arc<str>>,
        orchestrator: Arc<Orchestrator>,
        conversations_dir: PathBuf,
        devices_path: PathBuf,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            auth_key: auth_key.into(),
            server_name: server_name.into(),
            orchestrator,
            conversations_dir: Arc::new(conversations_dir),
            devices: Arc::new(Mutex::new(HashMap::new())),
            devices_path: Arc::new(devices_path),
            revocation_check_interval: DEFAULT_REVOCATION_CHECK_INTERVAL,
            tls: None,
            web_ui: None,
            transcriber: None,
        })
    }

    /// Makes this hub TLS-only (P36): `Hello` and everything after it only over `wss://`. The same
    /// port still answers a plain `ws://` upgrade on `DISCOVER_PATH`, and only with `DiscoverAck`
    /// (pointing at the `wss://` URL) — any other plain upgrade gets `426 Upgrade Required` before
    /// the client can send a `Hello`, so a misconfigured client never puts its key on the wire.
    pub fn with_tls(mut self, tls: HubTls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Also serves a web interface (P78) on this same port: a plain HTTP request (anything without
    /// `Upgrade: websocket`) gets a file from `assets`. Without this, the hub stays WebSocket-only.
    pub fn with_web_ui(mut self, assets: Arc<dyn WebAssets>) -> Self {
        self.web_ui = Some(assets);
        self
    }

    /// Answers `Transcribe` (voice input, P78) with `transcriber`. Without it, every `Transcribe`
    /// gets a `TranscriptionError`.
    pub fn with_transcriber(mut self, transcriber: Arc<dyn Transcriber>) -> Self {
        self.transcriber = Some(transcriber);
        self
    }

    /// Overrides `DEFAULT_REVOCATION_CHECK_INTERVAL` — tests use a short one so a revocation shows
    /// up without waiting seconds.
    pub fn with_revocation_check_interval(mut self, interval: Duration) -> Self {
        self.revocation_check_interval = interval;
        self
    }

    /// Actual bound address — useful when `bind` was called with port 0 (OS-assigned), e.g. in tests.
    pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    /// Accepts connections forever, one task per connection. Only returns on a listener-level
    /// error (a single connection's failure never brings the server down) — the standalone
    /// `warden-server` binary's normal mode, never asked to stop gracefully.
    pub async fn serve(self) -> anyhow::Result<()> {
        self.serve_until(std::future::pending()).await
    }

    /// Same accept loop as `serve`, but also races a `shutdown` future — resolving it stops the
    /// loop and drops the listener (freeing the port) instead of running forever. Used by the
    /// desktop's embedded server (Fase 9.1 follow-up, "virar o hub") so switching the toggle off
    /// actually releases the port instead of leaking a task that accepts forever.
    pub async fn serve_until(self, shutdown: impl std::future::Future<Output = ()>) -> anyhow::Result<()> {
        let listener = self.listener;
        let secure_url = match &self.tls {
            Some(tls) => tls.secure_url(listener.local_addr()?.port()).map(Arc::from),
            None => None,
        };
        let ctx = ConnectionContext {
            auth_key: self.auth_key,
            server_name: self.server_name,
            orchestrator: self.orchestrator,
            conversations_dir: self.conversations_dir,
            devices: self.devices,
            devices_path: self.devices_path,
            revocation_check_interval: self.revocation_check_interval,
            secure_url,
            transcriber: self.transcriber,
        };
        let tls = self.tls;
        let web_ui = self.web_ui;
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, peer) = accepted?;
                    let ctx = ctx.clone();
                    let tls = tls.clone();
                    let web_ui = web_ui.clone();
                    tokio::spawn(async move {
                        if let Err(err) = route_connection(stream, peer, tls, web_ui, ctx).await {
                            eprintln!("warden-server: connection from {peer} ended with error: {err:#}");
                        }
                    });
                }
                _ = &mut shutdown => return Ok(()),
            }
        }
    }
}

/// Everything a connection handler needs that isn't specific to one connection — grouped so
/// `handle_connection` takes one argument instead of cloning/threading each field by hand.
#[derive(Clone)]
struct ConnectionContext {
    auth_key: Arc<str>,
    server_name: Arc<str>,
    orchestrator: Arc<Orchestrator>,
    conversations_dir: Arc<PathBuf>,
    devices: DeviceRegistry,
    devices_path: Arc<PathBuf>,
    revocation_check_interval: Duration,
    /// `DiscoverAck.secure_url` — set only on a TLS hub that knows its public name.
    secure_url: Option<Arc<str>>,
    transcriber: Option<Arc<dyn Transcriber>>,
}

/// Picks the transport for a fresh TCP connection. Without TLS, everything is plain `ws://` as
/// before. With TLS, the first byte decides (see `TLS_HANDSHAKE_RECORD`): a TLS handshake goes on
/// to the full protocol, a plain upgrade only gets `serve_plain_discover`. With a web UI (P78), a
/// request that isn't a WebSocket upgrade gets a page instead (see `serve_web_or_ws`).
async fn route_connection(stream: TcpStream, peer: SocketAddr, tls: Option<HubTls>, web_ui: Option<Arc<dyn WebAssets>>, ctx: ConnectionContext) -> anyhow::Result<()> {
    let Some(tls) = tls else {
        return serve_web_or_ws(stream, peer, web_ui, ctx).await;
    };

    let mut first = [0u8; 1];
    let read = tokio::time::timeout(TLS_ACCEPT_TIMEOUT, stream.peek(&mut first)).await??;
    if read == 1 && first[0] == TLS_HANDSHAKE_RECORD {
        let stream = tokio::time::timeout(TLS_ACCEPT_TIMEOUT, tls.acceptor.accept(stream)).await??;
        return serve_web_or_ws(stream, peer, web_ui, ctx).await;
    }
    if web_ui.is_none() {
        return serve_plain_discover(stream, ctx).await;
    }
    // Plain bytes to a TLS hub with a web UI: a browser typing `http://` gets sent to `https://`,
    // while a discovery probe (a WebSocket upgrade) still goes where it always did.
    let mut stream = stream;
    let Some(head) = tokio::time::timeout(web_ui::HEAD_TIMEOUT, web_ui::read_request_head(&mut stream)).await?? else {
        return Ok(());
    };
    if head.is_websocket_upgrade {
        serve_plain_discover(Rewind::new(head.raw, stream), ctx).await
    } else {
        web_ui::redirect_to_https(&mut stream, &head, ctx.secure_url.as_deref()).await?;
        Ok(())
    }
}

/// The full protocol over `stream` — preceded, when this hub has a web UI, by a look at the request
/// head: a WebSocket upgrade continues as before (its bytes handed back via `Rewind`), anything else
/// is answered as a page request and the connection ends there.
async fn serve_web_or_ws<S: Transport>(mut stream: S, peer: SocketAddr, web_ui: Option<Arc<dyn WebAssets>>, ctx: ConnectionContext) -> anyhow::Result<()> {
    let Some(assets) = web_ui else {
        let ws = tokio_tungstenite::accept_async(stream).await?;
        return handle_connection(ws, peer, ctx).await;
    };
    let Some(head) = tokio::time::timeout(web_ui::HEAD_TIMEOUT, web_ui::read_request_head(&mut stream)).await?? else {
        return Ok(());
    };
    if head.is_websocket_upgrade {
        let ws = tokio_tungstenite::accept_async(Rewind::new(head.raw, stream)).await?;
        handle_connection(ws, peer, ctx).await
    } else {
        web_ui::serve(&mut stream, &head, assets.as_ref()).await?;
        Ok(())
    }
}

/// A plain `ws://` connection to a TLS-only hub: upgraded only on `DISCOVER_PATH` (refused with
/// `426 Upgrade Required` anywhere else, so no `Hello` can follow), and answers a single
/// `Discover` with where to connect instead.
async fn serve_plain_discover<S: Transport>(stream: S, ctx: ConnectionContext) -> anyhow::Result<()> {
    // The error type is tungstenite's `Callback` signature, not ours to shrink.
    #[allow(clippy::result_large_err)]
    let only_discover = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        if request.uri().path() == DISCOVER_PATH {
            return Ok(response);
        }
        let mut refusal = ErrorResponse::new(Some("this hub only accepts encrypted connections (wss://)".to_string()));
        *refusal.status_mut() = StatusCode::UPGRADE_REQUIRED;
        Err(refusal)
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, only_discover).await else {
        return Ok(());
    };
    let (mut sink, mut stream) = ws.split();
    if let Some(Ok(Message::Text(text))) = stream.next().await {
        if let Ok(ClientMessage::Discover) = serde_json::from_str::<ClientMessage>(&text) {
            send(&mut sink, &discover_ack(&ctx)).await?;
        }
    }
    sink.send(Message::Close(None)).await?;
    Ok(())
}

fn discover_ack(ctx: &ConnectionContext) -> ServerMessage {
    ServerMessage::DiscoverAck { server_name: ctx.server_name.to_string(), secure_url: ctx.secure_url.as_deref().map(str::to_string) }
}

async fn handle_connection<S: Transport>(ws: WebSocketStream<S>, peer: SocketAddr, ctx: ConnectionContext) -> anyhow::Result<()> {
    let discover_reply = discover_ack(&ctx);
    let ConnectionContext {
        auth_key,
        server_name,
        orchestrator,
        conversations_dir,
        devices,
        devices_path,
        revocation_check_interval,
        secure_url: _,
        transcriber,
    } = ctx;
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
            device_token,
            tools,
        }) => (device_id, device_name, provided, device_token, tools),
        // Fase 9.1 (redefined): an unauthenticated presence probe from a LAN-discovery sweep —
        // answered and closed right here, before any of the Hello/auth-key/device-registry
        // machinery below runs. Never becomes a "connected device".
        Ok(ClientMessage::Discover) => {
            send(&mut sink, &discover_reply).await?;
            sink.send(Message::Close(None)).await?;
            return Ok(());
        }
        Ok(_) => {
            eprintln!("warden-server: {peer} didn't send Hello first, closing");
            return Ok(());
        }
        Err(err) => {
            eprintln!("warden-server: {peer} sent an unparseable first frame: {err}");
            return Ok(());
        }
    };
    let (device_id, device_name, provided_key, device_token, tools) = hello;

    // P36: the shared key only pairs; a paired device authenticates with its own token. The
    // pairing status itself (Pending/Approved) stays silent here — a `Pending` device still gets a
    // normal `HelloAck` and can chat; only `CallDeviceTool` checks for `Approved` (Fase 9.3).
    let pairing_key_ok = !provided_key.is_empty() && provided_key.as_str() == auth_key.as_ref();
    let store = PairingStore::new(devices_path.as_ref().clone());
    let issued_token = match store.authenticate(&device_id, &device_name, device_token.as_deref(), pairing_key_ok) {
        Ok(Ok(outcome)) => outcome.issued_token,
        Ok(Err(rejection)) => {
            eprintln!("warden-server: rejected Hello from '{device_id}' at {peer}: {rejection}");
            return reject(&mut sink, &rejection.to_string()).await;
        }
        Err(err) => {
            eprintln!("warden-server: failed to read the pairing registry for '{device_id}': {err:#}");
            return reject(&mut sink, "server could not check this device's pairing").await;
        }
    };

    eprintln!("warden-server: {device_name} ({device_id}) connected from {peer}");

    // Every device's conversations, for `RequestUsage`'s hub-wide totals.
    let conversations_root = conversations_dir.clone();
    // P78: this device's own conversations directory — resolved (and its pre-P78 conversation
    // migrated into it) once here, before any of its requests can touch it.
    let conversations_dir = match device_conversations_dir(&conversations_dir, &device_id) {
        Ok(dir) => Arc::new(dir),
        Err(err) => {
            eprintln!("warden-server: failed to prepare {device_id}'s conversations directory: {err:#}");
            return reject(&mut sink, "server could not open this device's conversations").await;
        }
    };

    send(&mut sink, &ServerMessage::HelloAck {
        server_name: server_name.to_string(),
        device_token: issued_token,
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
            // P36: an `AuthError` after the handshake only ever means "revoked" — close right
            // behind it instead of leaving the socket open until every sender is gone.
            if let ServerMessage::AuthError { reason } = msg {
                let _ = sink.send(Message::Close(Some(CloseFrame { code: CloseCode::Policy, reason: reason.into() }))).await;
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
    //
    // P42: a client's advertised name can collide with the shared Orchestrator's own tools (vault,
    // shell, SSH, MCP servers) — the original real case was the phone's first `list_files`/
    // `read_file` colliding with the vault's own tools of the same name, which broke the next model
    // call with a provider-side "duplicate function" error rather than anything clear from Warden.
    // Deduped the same way `warden-bootstrap::register_mcp_tools` dedupes an MCP server's tools
    // (P46) — only renamed on a real collision, namespaced by this device's id, via the shared
    // `warden_core::tool::dedupe_tool_name`/`rename_tool`. `RemoteTool::call` sends the request
    // using its own internal spec, never what this wrapper reports, so the client is never told
    // about the rename — it keeps answering to the name it always advertised.
    let orchestrator: Arc<Orchestrator> = if tools.is_empty() {
        orchestrator
    } else {
        let mut per_connection = (*orchestrator).clone();
        for spec in tools {
            let original = spec.name.clone();
            let existing: Vec<String> = per_connection.tools().iter().map(|t| t.spec().name).collect();
            let resolved = warden_core::tool::dedupe_tool_name(&existing, &device_id, &original);
            let remote = Arc::new(RemoteTool::new(spec, tool_channel.clone(), REMOTE_TOOL_TIMEOUT));
            if resolved == original {
                per_connection.register_tool(remote);
            } else {
                eprintln!(
                    "warden-server: {device_id}'s tool '{original}' collides with an already-registered tool — \
                     renamed to '{resolved}'\n"
                );
                per_connection.register_tool(warden_core::tool::rename_tool(remote, resolved));
            }
        }
        Arc::new(per_connection)
    };

    // P36: `revoke` must also end a connection that's already open — it happens in another
    // process (`warden-server devices revoke`, the desktop's Workspace screen), so the only signal
    // is the registry file itself, re-checked on a timer.
    let mut revocation_check = tokio::time::interval(revocation_check_interval);
    revocation_check.tick().await;
    loop {
        let frame = tokio::select! {
            frame = stream.next() => frame,
            _ = revocation_check.tick() => {
                if matches!(store.status(&device_id), Ok(Some(PairingStatus::Revoked))) {
                    eprintln!("warden-server: {device_id} was revoked, closing its connection");
                    let _ = tx.send(ServerMessage::AuthError { reason: AuthRejection::Revoked.to_string() });
                    break;
                }
                continue;
            }
        };
        let Some(frame) = frame else { break };
        match frame? {
            Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::Ping { nonce }) => {
                    let _ = tx.send(ServerMessage::Pong { nonce });
                }
                Ok(ClientMessage::Chat { message, conversation_id, attachments }) => {
                    let checked = resolve_conversation_id(conversation_id.clone())
                        .and_then(|id| validate_attachments(&attachments).map(|()| id));
                    let conversation_id = match checked {
                        Ok(id) => id,
                        Err(message) => {
                            let _ = tx.send(ServerMessage::ChatError { message, conversation_id, spend_limit_id: None });
                            continue;
                        }
                    };
                    // Spending limits (P4) are counted per connected device.
                    let orchestrator = orchestrator.with_spend_context(SpendContext::new("server").with_user(device_id.clone()));
                    let conversations_dir = conversations_dir.clone();
                    let reply_tx = tx.clone();
                    tokio::spawn(async move {
                        let reply = match warden_bootstrap::handle_turn(
                            &orchestrator,
                            &conversations_dir,
                            &conversation_id,
                            &title_seed(&message, &attachments),
                            &message,
                            attachments,
                        )
                        .await
                        {
                            Ok(outcome) => ServerMessage::ChatResponse {
                                content: outcome.content,
                                usage: outcome.usage,
                                attachments: outcome.attachments,
                                conversation_id: Some(conversation_id),
                            },
                            Err(err) => ServerMessage::ChatError {
                                message: format!("{err:#}"),
                                conversation_id: Some(conversation_id),
                                spend_limit_id: spend_limit_id(&err),
                            },
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
                Ok(message @ (ClientMessage::ListSkills { .. } | ClientMessage::SaveSkill { .. } | ClientMessage::DeleteSkill { .. })) => {
                    // Short local file I/O, answered inline (no spawn) — P72.
                    let store = SkillStore::new(orchestrator.vault().clone());
                    if let Some(reply) = handle_skill_request(&store, message) {
                        let _ = tx.send(reply);
                    }
                }
                Ok(ClientMessage::RequestHistory { request_id, limit, conversation_id }) => {
                    // P40 — one small file read, answered inline like the skills requests. Inline
                    // also means a `Chat` sent right after this request can never land in the
                    // reply: that turn is only saved once its (spawned) model call finishes.
                    let _ = tx.send(handle_history_request(&conversations_dir, request_id, limit, conversation_id));
                }
                Ok(ClientMessage::Transcribe { request_id, audio }) => {
                    // P78 — a Whisper call takes seconds, so it runs off the reader loop like `Chat`.
                    let transcriber = transcriber.clone();
                    let reply_tx = tx.clone();
                    tokio::spawn(async move {
                        let _ = reply_tx.send(handle_transcribe(transcriber.as_deref(), request_id, audio).await);
                    });
                }
                Ok(
                    message @ (ClientMessage::ListVaultFiles { .. }
                    | ClientMessage::ReadVaultNote { .. }
                    | ClientMessage::SaveVaultNote { .. }
                    | ClientMessage::DeleteVaultNote { .. }
                    | ClientMessage::SearchVault { .. }),
                ) => {
                    // P78 — listing and searching walk the whole vault, so this runs on the
                    // blocking pool instead of holding up this connection's reader loop.
                    let vault = orchestrator.vault().clone();
                    let reply_tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Some(reply) = handle_vault_request(&vault, message) {
                            let _ = reply_tx.send(reply);
                        }
                    });
                }
                Ok(ClientMessage::RequestUsage { request_id, tz_offset_minutes }) => {
                    // P78 — reads every device's conversations, so off the reader loop.
                    let root = conversations_root.clone();
                    let pairing = PairingStore::new(devices_path.as_ref().clone());
                    let guard = orchestrator.spend_guard().cloned();
                    let reply_tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = reply_tx.send(handle_usage_request(&root, &pairing, guard.as_deref(), request_id, tz_offset_minutes));
                    });
                }
                Ok(ClientMessage::ExtendLimit { request_id, limit_id }) => {
                    let guard = orchestrator.spend_guard().cloned();
                    let reply_tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = reply_tx.send(handle_extend_limit(guard.as_deref(), request_id, &limit_id));
                    });
                }
                Ok(message @ (ClientMessage::ListConversations { .. } | ClientMessage::RenameConversation { .. } | ClientMessage::DeleteConversation { .. })) => {
                    // P78 — small file I/O, answered inline like `RequestHistory`.
                    if let Some(reply) = handle_conversation_request(&conversations_dir, message) {
                        let _ = tx.send(reply);
                    }
                }
                Ok(ClientMessage::Goodbye { reason }) => {
                    eprintln!("warden-server: {device_id} said goodbye ({reason:?})");
                    break;
                }
                Ok(ClientMessage::Hello { .. }) => {
                    eprintln!("warden-server: {device_id} sent a second Hello, ignoring");
                }
                Ok(ClientMessage::Discover) => {
                    eprintln!("warden-server: {device_id} sent Discover after Hello, ignoring");
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
    // `tool_channel` and this connection's own `Orchestrator` (its `RemoteTool`s) hold `tx` clones
    // too — dropped here so the writer task actually ends once in-flight `Chat` tasks finish,
    // instead of waiting on a sender that lives as long as this function.
    drop((tx, tool_channel, orchestrator));
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

/// Turns a `Hello` down: `AuthError` with `reason`, then a policy close.
async fn reject<S: Transport>(sink: &mut WsSink<S>, reason: &str) -> anyhow::Result<()> {
    send(sink, &ServerMessage::AuthError { reason: reason.to_string() }).await?;
    sink.send(Message::Close(Some(CloseFrame { code: CloseCode::Policy, reason: reason.to_string().into() }))).await?;
    Ok(())
}

async fn send<S: Transport>(sink: &mut WsSink<S>, msg: &ServerMessage) -> anyhow::Result<()> {
    let json = serde_json::to_string(msg)?;
    sink.send(Message::Text(json.into())).await?;
    Ok(())
}

/// Resolution order: an explicit name (e.g. `--server-name`, or a value typed into the desktop's
/// embedded-server settings) > `WARDEN_SERVER_NAME` > OS hostname > a fixed literal — same
/// fallback shape `crates/warden-sync/src/pairing/join.rs::device_name()` uses, so a hub with
/// nothing configured still answers a discovery sweep with something recognizable instead of an
/// empty string. Shared by the standalone `warden-server` binary (`main.rs`) and the desktop's
/// embedded server (Fase 9.1 follow-up) so both resolve a name the same way.
pub fn resolve_server_name(explicit: Option<String>) -> String {
    explicit
        .or_else(|| std::env::var("WARDEN_SERVER_NAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "warden-server".to_string())
}

#[cfg(test)]
mod name_tests {
    use super::resolve_server_name;

    #[test]
    fn explicit_name_wins_over_everything() {
        assert_eq!(resolve_server_name(Some("My Hub".to_string())), "My Hub");
    }
}
