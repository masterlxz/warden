use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message};
use tokio_tungstenite::WebSocketStream;
use warden_core::orchestrator::Orchestrator;
use warden_core::skill::SkillStore;
use warden_core::tool::ToolSpec;
use warden_core::spend::SpendContext;
use warden_bootstrap::auto_sync::SyncRunner;
use warden_bootstrap::tasks::TaskStore;
use warden_bootstrap::{build_model_for, load_config_from_path, scope_to_agent, AgentExtras, TurnAgent};

use crate::approval::WsApprover;
use crate::chat_input::{handle_transcribe, title_seed, validate_attachments, Transcriber};
use crate::conversations::{handle_conversation_request, handle_history_request, resolve_conversation_id, ConversationDirs};
use crate::device_registry::{AuthRejection, PairingProof, PairingStatus, PairingStore};
use crate::people::{member_orchestrator, member_refusal, member_settings_view, migrate_device_conversations, password_gate, user_info, MemberSpace, Person};
use crate::user_admin::{handle_change_password, handle_list_users, handle_user_change, UserChange};
use crate::devices::{handle_list_devices, handle_set_device_status};
use crate::skills::handle_skill_request;
use crate::usage::{handle_extend_limit, handle_usage_request, spend_limit_id};
use crate::vault::handle_vault_request;
use crate::remote_tool::{RemoteTool, RemoteToolChannel, DEFAULT_TIMEOUT as REMOTE_TOOL_TIMEOUT};
use crate::node_tools::NodeToolFactory;
use crate::nodes::{handle_list_nodes, handle_set_node_access, ConnectedNode, HubNodeModelRouter, ModelChannel, NodeRegistry};
use crate::scheduler::{scheduler_loop, TaskRunner, DEFAULT_TICK as DEFAULT_TASK_TICK};
use crate::task_admin::{handle_list_tasks, handle_task_change, TaskAccess, TaskChange};
use crate::sync::{handle_sync_action, handle_sync_status, SyncAccess};
use crate::settings::{handle_request_settings, handle_save_settings, is_secure, SettingsAccess, SettingsHost, SharedOrchestrator};
use crate::tls::HubTls;
use crate::api_key_admin::{handle_api_key_change, handle_list_api_keys, ApiKeyChange};
use crate::api_keys::ApiKeyStore;
use crate::openai_api::{self, ApiContext};
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
/// As of Fase 7.3, hosts a real `Orchestrator` and answers `Chat` messages with it. As of Fase
/// 9.3/9.4, also keeps a registry of every connected device and routes `CallDeviceTool` from one
/// connection to a specific other one (no client uses it today — see `lib.rs`).
pub struct Server {
    listener: TcpListener,
    auth_key: Arc<str>,
    server_name: Arc<str>,
    orchestrator: SharedOrchestrator,
    conversations_dir: Arc<PathBuf>,
    devices: DeviceRegistry,
    devices_path: Arc<PathBuf>,
    revocation_check_interval: Duration,
    tls: Option<HubTls>,
    web_ui: Option<Arc<dyn WebAssets>>,
    transcriber: Option<Arc<dyn Transcriber>>,
    settings: Option<Arc<dyn SettingsHost>>,
    sync: Option<Arc<SyncRunner>>,
    /// Whether this server runs `sync`'s rounds on its own loop (the standalone hub), or only
    /// answers for a runner someone else loops (the desktop's embedded hub).
    sync_loop: Option<Duration>,
    /// Where the Warden API's keys live (P12). `None`: no API on this hub.
    api_keys: Option<Arc<PathBuf>>,
    /// Scheduled tasks (P92): their conversations, which every device lists, and whether this hub
    /// also runs them on schedule (`--run-tasks`).
    tasks: Option<TaskRunner>,
    task_tick: Duration,
    /// Conversations changed outside any one connection (a task ran): every connection hears it.
    changes: broadcast::Sender<String>,
    /// Who is connected as a node (P93), for the node tools and the screens.
    nodes: NodeRegistry,
    /// Where node calls are logged; `None` in tests that don't care.
    node_audit: Option<PathBuf>,
    /// Where members keep their own vaults (P84). `None`: nobody but the root can sign in.
    users_dir: Option<PathBuf>,
}

/// How often an open connection re-reads the pairing registry to notice it was revoked (P36).
pub const DEFAULT_REVOCATION_CHECK_INTERVAL: Duration = Duration::from_secs(5);

impl Server {
    pub async fn bind(
        addr: SocketAddr,
        auth_key: impl Into<Arc<str>>,
        server_name: impl Into<Arc<str>>,
        orchestrator: impl Into<SharedOrchestrator>,
        conversations_dir: PathBuf,
        devices_path: PathBuf,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            auth_key: auth_key.into(),
            server_name: server_name.into(),
            orchestrator: orchestrator.into(),
            conversations_dir: Arc::new(conversations_dir),
            devices: Arc::new(Mutex::new(HashMap::new())),
            devices_path: Arc::new(devices_path),
            revocation_check_interval: DEFAULT_REVOCATION_CHECK_INTERVAL,
            tls: None,
            web_ui: None,
            transcriber: None,
            settings: None,
            sync: None,
            sync_loop: None,
            api_keys: None,
            tasks: None,
            task_tick: DEFAULT_TASK_TICK,
            changes: broadcast::channel(64).0,
            nodes: NodeRegistry::default(),
            node_audit: warden_bootstrap::default_node_audit_log_path(),
            users_dir: None,
        })
    }

    /// Lets the members in `[[users]]` (P84) sign in with their password, each with their own vault
    /// under `dir` (`warden_bootstrap::users::default_users_dir`). Needs `with_settings` too, since
    /// that's where the members are read from.
    pub fn with_users_dir(mut self, dir: PathBuf) -> Self {
        self.users_dir = Some(dir);
        self
    }

    /// The nodes connected to this hub (P93) — the desktop's Workspace screen reads its embedded hub's.
    pub fn node_registry(&self) -> NodeRegistry {
        self.nodes.clone()
    }

    /// Logs node calls to `path` instead of the default file — tests keep it in their temp dir.
    pub fn with_node_audit(mut self, path: Option<PathBuf>) -> Self {
        self.node_audit = path;
        self
    }

    /// Scheduled tasks (P92): every device lists their conversations from `store`, and with `run`
    /// this hub also runs them — re-reading `[[tasks]]` from the settings file, so it needs
    /// `with_settings` too. Off by default: the config syncs, and only one hub should run them.
    pub fn with_tasks(mut self, store: TaskStore, run: bool) -> Self {
        self.tasks = Some(TaskRunner::new(store, self.changes.clone(), run));
        self
    }

    /// The runner `with_tasks` set up, for a "run now" from outside a connection (the desktop's
    /// Tasks screen) that must not overlap a scheduled run of the same task.
    pub fn task_runner(&self) -> Option<TaskRunner> {
        self.tasks.clone()
    }

    /// Where this hub announces a conversation that changed outside any connection — a task's run.
    /// The desktop sends one after a "run now" of its own, so the embedded hub's devices hear it.
    pub fn conversation_changes(&self) -> broadcast::Sender<String> {
        self.changes.clone()
    }

    /// Overrides how often the scheduler looks for due tasks — tests use a short one.
    pub fn with_task_tick(mut self, tick: Duration) -> Self {
        self.task_tick = tick;
        self
    }

    /// Serves the Warden API (P12) on this same port: `/v1/models` and `/v1/chat/completions`,
    /// OpenAI-compatible, for anyone holding a key from `api_keys_path` (see `api_keys.rs`).
    pub fn with_api(mut self, api_keys_path: PathBuf) -> Self {
        self.api_keys = Some(Arc::new(api_keys_path));
        self
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

    /// Answers `RequestSettings`/`SaveSettings` (P78) through `host`, reloading this hub's
    /// orchestrator on a save. Without it, both get a `SettingsError`.
    pub fn with_settings(mut self, host: Arc<dyn SettingsHost>) -> Self {
        self.settings = Some(host);
        self
    }

    /// Answers `RequestSyncStatus`/`SyncAction` (P61) with `runner`. With `loop_every`, this server
    /// also runs a round every so often for as long as it serves, and reloads its orchestrator when
    /// one brings a new `config.toml` — the standalone hub. The desktop's embedded hub passes `None`:
    /// the desktop already loops the same runner.
    pub fn with_sync(mut self, runner: Arc<SyncRunner>, loop_every: Option<Duration>) -> Self {
        self.sync = Some(runner);
        self.sync_loop = loop_every;
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
        let mut ctx = ConnectionContext {
            auth_key: self.auth_key,
            server_name: self.server_name,
            orchestrator: self.orchestrator,
            conversations_dir: self.conversations_dir,
            devices: self.devices,
            devices_path: self.devices_path,
            revocation_check_interval: self.revocation_check_interval,
            secure_url,
            transcriber: self.transcriber,
            settings: self.settings,
            settings_lock: Arc::new(tokio::sync::Mutex::new(())),
            sync: self.sync,
            api_keys: self.api_keys,
            tasks: self.tasks,
            changes: self.changes.clone(),
            nodes: self.nodes.clone(),
            node_tools: None,
            users_dir: self.users_dir.map(Arc::new),
        };
        // P84: conversations are a person's, not a device's — every device's move to the root's,
        // once, before any connection can read them.
        match migrate_device_conversations(&ctx.conversations_dir) {
            Ok(0) => {}
            Ok(moved) => eprintln!("warden-server: moved {moved} conversation(s) from each device's folder into the owner's"),
            Err(err) => eprintln!("warden-server: failed to move the devices' conversations into the owner's: {err:#}"),
        }
        // P93: the node tools join the hub's orchestrator — chat, the Warden API and scheduled tasks
        // all get them. They read `[[nodes]]` from the settings file, so a hub without one has none.
        if let Some(settings) = &ctx.settings {
            let factory = NodeToolFactory::new(self.nodes.clone(), settings.config_path(), ctx.devices_path.as_ref().clone(), self.node_audit.clone());
            ctx.orchestrator.set_extra_tools(factory.fixed_tools());
            ctx.orchestrator.set_dynamic_tools(factory.mcp_tools());
            ctx.node_tools = Some(factory);
            // Fatia 3: a `kind = "node"` provider in this process answers through this hub.
            warden_bootstrap::node_model::set_node_model_router(Some(Arc::new(HubNodeModelRouter {
                registry: self.nodes.clone(),
                config_path: settings.config_path(),
                devices_path: ctx.devices_path.as_ref().clone(),
            })));
        }
        let runs_tasks = ctx.tasks.as_ref().is_some_and(TaskRunner::runs_here);
        let _scheduler = match (&ctx.tasks, runs_tasks, &ctx.settings) {
            (Some(runner), true, Some(settings)) => {
                Some(AbortOnDrop(tokio::spawn(scheduler_loop(runner.clone(), settings.clone(), ctx.orchestrator.clone(), self.task_tick))))
            }
            (Some(_), true, None) => {
                eprintln!("warden-server: scheduled tasks need a config file to read them from — not running any");
                None
            }
            _ => None,
        };
        let _sync_loop = match (&ctx.sync, self.sync_loop) {
            (Some(runner), Some(every)) => Some(AbortOnDrop(tokio::spawn(sync_loop(runner.clone(), every, ctx.settings.clone(), ctx.orchestrator.clone())))),
            _ => None,
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
    orchestrator: SharedOrchestrator,
    conversations_dir: Arc<PathBuf>,
    devices: DeviceRegistry,
    devices_path: Arc<PathBuf>,
    revocation_check_interval: Duration,
    /// `DiscoverAck.secure_url` — set only on a TLS hub that knows its public name.
    secure_url: Option<Arc<str>>,
    transcriber: Option<Arc<dyn Transcriber>>,
    settings: Option<Arc<dyn SettingsHost>>,
    /// One settings save at a time on this hub.
    settings_lock: Arc<tokio::sync::Mutex<()>>,
    sync: Option<Arc<SyncRunner>>,
    api_keys: Option<Arc<PathBuf>>,
    /// Scheduled tasks (P92): their conversations are listed next to every device's own.
    tasks: Option<TaskRunner>,
    changes: broadcast::Sender<String>,
    nodes: NodeRegistry,
    /// Rebuilds the nodes' MCP tools when one joins or leaves (fatia 2). `None` without a settings file.
    node_tools: Option<NodeToolFactory>,
    /// Where members' own vaults live (P84); `None` when members can't sign in here.
    users_dir: Option<Arc<PathBuf>>,
}

impl ConnectionContext {
    fn api(&self) -> ApiContext {
        ApiContext { orchestrator: self.orchestrator.clone(), settings: self.settings.clone(), keys_path: self.api_keys.clone() }
    }
}

/// Off the reader loop: a wrong key waits a second under the settings lock.
#[allow(clippy::too_many_arguments)]
fn spawn_api_key_change(
    api_keys: &Option<Arc<PathBuf>>,
    settings: &Option<Arc<dyn SettingsHost>>,
    lock: &Arc<tokio::sync::Mutex<()>>,
    auth_key: &Arc<str>,
    tx: &mpsc::UnboundedSender<ServerMessage>,
    request_id: u64,
    pairing_key: String,
    change: ApiKeyChange,
) {
    let store = api_keys.as_deref().map(|path| ApiKeyStore::new(path.clone()));
    let (settings, lock, auth_key, reply_tx) = (settings.clone(), lock.clone(), auth_key.clone(), tx.clone());
    tokio::spawn(async move {
        let reply = handle_api_key_change(store.as_ref(), settings.as_deref(), &lock, &auth_key, request_id, &pairing_key, change).await;
        let _ = reply_tx.send(reply);
    });
}

/// Puts the connected nodes' MCP tools on the hub's orchestrator — after a node joined or left.
fn refresh_node_mcp_tools(shared: &SharedOrchestrator, factory: Option<&NodeToolFactory>) {
    if let Some(factory) = factory {
        shared.set_dynamic_tools(factory.mcp_tools());
    }
}

fn describe_offer(offer: &warden_server_protocol::protocol::NodeOfferDto) -> String {
    let mut parts = Vec::new();
    if offer.shell {
        parts.push("shell");
    }
    if offer.files {
        parts.push("files");
    }
    if !offer.mcp_tools.is_empty() {
        parts.push("MCP tools");
    }
    if !offer.models.is_empty() {
        parts.push("models");
    }
    if parts.is_empty() {
        "nothing".to_string()
    } else {
        parts.join(" and ")
    }
}

/// Runs a people change (P84) off the reader loop: a wrong key waits a second under the settings lock.
#[allow(clippy::too_many_arguments)]
fn spawn_user_change(
    settings: &Option<Arc<dyn SettingsHost>>,
    devices_path: &Arc<PathBuf>,
    lock: &Arc<tokio::sync::Mutex<()>>,
    auth_key: &Arc<str>,
    tx: &mpsc::UnboundedSender<ServerMessage>,
    request_id: u64,
    pairing_key: String,
    change: UserChange,
) {
    let (settings, lock, auth_key, reply_tx) = (settings.clone(), lock.clone(), auth_key.clone(), tx.clone());
    let pairing = PairingStore::new(devices_path.as_ref().clone());
    tokio::spawn(async move {
        let reply = handle_user_change(settings.as_deref(), &pairing, &lock, &auth_key, request_id, &pairing_key, change).await;
        let _ = reply_tx.send(reply);
    });
}

/// Off the reader loop, like the API keys: a wrong key waits a second under the settings lock.
#[allow(clippy::too_many_arguments)]
fn spawn_task_change(
    tasks: &Option<TaskRunner>,
    settings: &Option<Arc<dyn SettingsHost>>,
    shared: &SharedOrchestrator,
    lock: &Arc<tokio::sync::Mutex<()>>,
    auth_key: &Arc<str>,
    tx: &mpsc::UnboundedSender<ServerMessage>,
    request_id: u64,
    pairing_key: String,
    change: TaskChange,
) {
    let (tasks, settings, shared, lock, auth_key, reply_tx) = (tasks.clone(), settings.clone(), shared.clone(), lock.clone(), auth_key.clone(), tx.clone());
    tokio::spawn(async move {
        let access = TaskAccess { runner: tasks.as_ref(), settings: settings.as_deref(), shared: &shared, lock: &lock, auth_key: &auth_key };
        let _ = reply_tx.send(handle_task_change(&access, request_id, &pairing_key, change).await);
    });
}

/// Stops a task when the server that spawned it stops serving.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The standalone hub's sync loop (P61): `SyncRunner::run_loop`, plus a reload when a round
/// brings another device's `config.toml`.
async fn sync_loop(runner: Arc<SyncRunner>, every: Duration, settings: Option<Arc<dyn SettingsHost>>, shared: SharedOrchestrator) {
    runner
        .run_loop(every, |report| {
            let reload = report.config_updated();
            let (settings, shared) = (settings.clone(), shared.clone());
            async move {
                if reload {
                    crate::sync::reload_orchestrator(settings.as_deref(), &shared).await;
                }
            }
        })
        .await
}

/// Picks the transport for a fresh TCP connection. Without TLS, everything is plain `ws://` as
/// before. With TLS, the first byte decides (see `TLS_HANDSHAKE_RECORD`): a TLS handshake goes on
/// to the full protocol, a plain upgrade only gets `serve_plain_discover`. With a web UI (P78), a
/// request that isn't a WebSocket upgrade gets a page instead (see `serve_web_or_ws`).
async fn route_connection(stream: TcpStream, peer: SocketAddr, tls: Option<HubTls>, web_ui: Option<Arc<dyn WebAssets>>, ctx: ConnectionContext) -> anyhow::Result<()> {
    let Some(tls) = tls else {
        let secure = is_secure(false, peer.ip());
        return serve_web_or_ws(stream, peer, secure, web_ui, ctx).await;
    };

    let mut first = [0u8; 1];
    let read = tokio::time::timeout(TLS_ACCEPT_TIMEOUT, stream.peek(&mut first)).await??;
    if read == 1 && first[0] == TLS_HANDSHAKE_RECORD {
        let stream = tokio::time::timeout(TLS_ACCEPT_TIMEOUT, tls.acceptor.accept(stream)).await??;
        return serve_web_or_ws(stream, peer, true, web_ui, ctx).await;
    }
    // Plain bytes to a TLS hub: a browser typing `http://` (or an API client, P12) gets sent to
    // `https://`, while a discovery probe (a WebSocket upgrade) still goes where it always did.
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
/// `secure` is whether this connection may carry a new API key (see `settings::is_secure`).
async fn serve_web_or_ws<S: Transport>(mut stream: S, peer: SocketAddr, secure: bool, web_ui: Option<Arc<dyn WebAssets>>, ctx: ConnectionContext) -> anyhow::Result<()> {
    let Some(head) = tokio::time::timeout(web_ui::HEAD_TIMEOUT, web_ui::read_request_head(&mut stream)).await?? else {
        return Ok(());
    };
    if head.is_websocket_upgrade {
        let ws = tokio_tungstenite::accept_async(Rewind::new(head.raw, stream)).await?;
        return handle_connection(ws, peer, secure, ctx).await;
    }
    if head.path.starts_with(openai_api::API_PREFIX) {
        openai_api::serve(&mut stream, &head, &ctx.api()).await?;
        return Ok(());
    }
    match web_ui {
        Some(assets) => web_ui::serve(&mut stream, &head, assets.as_ref()).await?,
        None => web_ui::not_found(&mut stream, head.method == "HEAD").await?,
    }
    Ok(())
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

async fn handle_connection<S: Transport>(ws: WebSocketStream<S>, peer: SocketAddr, secure: bool, ctx: ConnectionContext) -> anyhow::Result<()> {
    let discover_reply = discover_ack(&ctx);
    let ConnectionContext {
        auth_key,
        server_name,
        orchestrator: shared_orchestrator,
        conversations_dir,
        devices,
        devices_path,
        revocation_check_interval,
        secure_url: _,
        transcriber,
        settings,
        settings_lock,
        sync,
        api_keys,
        tasks,
        changes,
        nodes,
        node_tools,
        users_dir,
    } = ctx;
    let tasks_dir = tasks.as_ref().map(|runner| Arc::new(runner.store().conversations_dir()));
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
            node,
            username,
            password,
        }) => (device_id, device_name, provided, device_token, tools, node, username.zip(password)),
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
    let (device_id, device_name, provided_key, device_token, tools, node_offer, credentials) = hello;

    // P36: the shared key only pairs; a paired device authenticates with its own token. The
    // pairing status itself (Pending/Approved) stays silent here — a `Pending` device still gets a
    // normal `HelloAck` and can chat; only `CallDeviceTool` checks for `Approved` (Fase 9.3).
    let pairing_key_ok = !provided_key.is_empty() && provided_key.as_str() == auth_key.as_ref();
    // P84: the members, read fresh — a member added or removed a moment ago counts.
    let members = match settings.as_deref() {
        Some(host) => load_config_from_path(&host.config_path(), false).map(|c| c.users).unwrap_or_default(),
        None => Vec::new(),
    };
    let proof = match &credentials {
        Some((username, password)) => match warden_bootstrap::users::authenticate_user(&members, username, password) {
            Some(user) if users_dir.is_some() => PairingProof::Member(user.id.clone()),
            Some(_) => return reject(&mut sink, "this hub doesn't host other people — sign in with the pairing key").await,
            // A device coming back with its token may still send what it paired with; the token decides.
            None if device_token.is_some() => PairingProof::Nothing,
            None => {
                eprintln!("warden-server: wrong username or password from '{device_id}' at {peer}");
                tokio::time::sleep(crate::settings::WRONG_KEY_DELAY).await;
                return reject(&mut sink, "wrong username or password").await;
            }
        },
        None if pairing_key_ok => PairingProof::PairingKey,
        None => PairingProof::Nothing,
    };
    let store = PairingStore::new(devices_path.as_ref().clone());
    let (issued_token, owner) = match store.authenticate_as(&device_id, &device_name, device_token.as_deref(), proof) {
        Ok(Ok(outcome)) => (outcome.issued_token, outcome.user),
        Ok(Err(rejection)) => {
            eprintln!("warden-server: rejected Hello from '{device_id}' at {peer}: {rejection}");
            return reject(&mut sink, &rejection.to_string()).await;
        }
        Err(err) => {
            eprintln!("warden-server: failed to read the pairing registry for '{device_id}': {err:#}");
            return reject(&mut sink, "server could not check this device's pairing").await;
        }
    };
    // Who this connection speaks for. A member's device whose member is gone is turned away.
    let (person, mut must_change_password, user) = match (&owner, users_dir.as_deref()) {
        (None, _) => (Person::Root, false, None),
        (Some(id), Some(users_dir)) => match members.iter().find(|u| &u.id == id) {
            Some(member) => (Person::Member(MemberSpace::new(member, users_dir, &conversations_dir)), member.must_change_password, Some(user_info(member))),
            None => return reject(&mut sink, "this person is no longer part of the workspace").await,
        },
        (Some(_), None) => return reject(&mut sink, "this hub doesn't host other people any more").await,
    };

    match &person {
        Person::Root => eprintln!("warden-server: {device_name} ({device_id}) connected from {peer}"),
        Person::Member(member) => eprintln!("warden-server: {device_name} ({device_id}) connected from {peer} as {}", member.id),
    }

    // Every person's conversations, for `RequestUsage`'s hub-wide totals.
    let conversations_root = conversations_dir.clone();
    // P84: a person's conversations, shared by all their devices; the scheduled tasks' (P92) only
    // for the root, who owns them.
    let conversation_dirs = Arc::new(match &person {
        Person::Root => ConversationDirs {
            device: warden_bootstrap::users::root_conversations_dir(&conversations_root),
            tasks: tasks_dir.as_deref().cloned(),
        },
        Person::Member(member) => ConversationDirs { device: member.conversations.clone(), tasks: None },
    });
    let conversations_dir = Arc::new(conversation_dirs.device.clone());
    let member = match &person {
        Person::Member(member) => Some(member.clone()),
        Person::Root => None,
    };

    send(&mut sink, &ServerMessage::HelloAck {
        server_name: server_name.to_string(),
        device_token: issued_token,
        user,
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

    // P92: a task that ran changed a conversation every device lists. Through a weak sender, so this
    // forwarder never keeps the writer alive; aborted when the connection ends.
    let mut hub_changes = changes.subscribe();
    // P84: those are the root's tasks; a member never hears about them.
    let hears_task_changes = member.is_none();
    let weak_tx = tx.downgrade();
    let _forward_changes = AbortOnDrop(tokio::spawn(async move {
        loop {
            match hub_changes.recv().await {
                Ok(_) if !hears_task_changes => continue,
                Ok(conversation_id) => {
                    let Some(tx) = weak_tx.upgrade() else { break };
                    let _ = tx.send(ServerMessage::ConversationsChanged { conversation_id });
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }));

    // Fase 9.3: every connected device is a valid routing target for `CallDeviceTool`, whether or
    // not it advertised any `Hello.tools` — registered before the loop starts so a routed call
    // arriving right after this device's own Hello can never race the registration.
    let tool_channel = RemoteToolChannel::new(tx.clone());
    devices.lock().unwrap().insert(device_id.clone(), tool_channel.clone());
    // P93: a node lends its shell, files, MCP tools and models to the hub's agents over this connection.
    let node_models = node_offer.as_ref().map(|_| ModelChannel::new(&tx));
    if let (Some(offer), Some(models)) = (node_offer, &node_models) {
        eprintln!("warden-server: {device_name} ({device_id}) is a node offering {}", describe_offer(&offer));
        nodes.connect(&device_id, ConnectedNode { name: device_name.clone(), offer, channel: tool_channel.clone(), models: models.clone() });
        refresh_node_mcp_tools(&shared_orchestrator, node_tools.as_ref());
    }

    // Fase 7.4: a client that advertised tools in Hello gets its own Orchestrator (cheap clone —
    // Orchestrator is Arc-backed) with a RemoteTool proxy per advertised spec, so the model can
    // invoke a capability that only exists on *this* device (mobile's file access, to start). A
    // client with nothing to advertise (tools empty) just reuses the shared, server-wide instance.
    // Name collisions are handled in `ConnectionOrchestrator::with_device_tools` (P42).
    //
    // P78: the hub's orchestrator can be swapped by a settings save, so this is rebuilt whenever the
    // shared one changes (see `ConnectionOrchestrator`).
    let mut orchestrator = ConnectionOrchestrator::new(shared_orchestrator.clone(), tools, tool_channel.clone(), device_id.clone());
    // P46: approvals for this device's turns come back on this connection.
    let approver = WsApprover::new(tx.clone());

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
        // A read error (the peer vanished without closing, e.g. a node that lost power) ends the
        // connection like a close does: the cleanup after the loop must still run, or calls waiting
        // on this device would sit out their whole timeout.
        let frame = match frame {
            Ok(frame) => frame,
            Err(err) => {
                eprintln!("warden-server: {device_id}'s connection broke: {err}");
                break;
            }
        };
        match frame {
            Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
                // P84: a member on a provisional password may only change it; a member never
                // reaches the hub's administration.
                Ok(message) if must_change_password && password_gate(&message).is_some() => {
                    if let Some(reply) = password_gate(&message) {
                        let _ = tx.send(reply);
                    }
                }
                Ok(message) if member.is_some() && member_refusal(&message).is_some() => {
                    if let Some(reply) = member_refusal(&message) {
                        let _ = tx.send(reply);
                    }
                }
                Ok(ClientMessage::ChangePassword { request_id, old_password, new_password }) => {
                    match &member {
                        None => {
                            let _ = tx.send(ServerMessage::UserError { request_id, message: "the workspace's owner signs in with the pairing key, which has no password to change".into(), auth_rejected: false });
                        }
                        Some(member) => {
                            let reply = handle_change_password(settings.as_deref(), &settings_lock, &member.id, request_id, &old_password, &new_password).await;
                            if matches!(reply, ServerMessage::PasswordChanged { .. }) {
                                must_change_password = false;
                            }
                            let _ = tx.send(reply);
                        }
                    }
                }
                Ok(ClientMessage::ListUsers { request_id }) => {
                    let _ = tx.send(handle_list_users(settings.as_deref(), request_id));
                }
                Ok(ClientMessage::SaveUser { request_id, pairing_key, id, name, is_new }) => {
                    let change = if is_new { UserChange::Create { id, name } } else { UserChange::Rename { id, name } };
                    spawn_user_change(&settings, &devices_path, &settings_lock, &auth_key, &tx, request_id, pairing_key, change);
                }
                Ok(ClientMessage::ResetPassword { request_id, pairing_key, id }) => {
                    spawn_user_change(&settings, &devices_path, &settings_lock, &auth_key, &tx, request_id, pairing_key, UserChange::ResetPassword { id });
                }
                Ok(ClientMessage::RemoveUser { request_id, pairing_key, id }) => {
                    spawn_user_change(&settings, &devices_path, &settings_lock, &auth_key, &tx, request_id, pairing_key, UserChange::Remove { id });
                }
                Ok(ClientMessage::Ping { nonce }) => {
                    let _ = tx.send(ServerMessage::Pong { nonce });
                }
                Ok(ClientMessage::Chat { message, conversation_id, attachments, agent_id }) => {
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
                    let base = orchestrator.current().with_spend_context(SpendContext::new("server").with_user(device_id.clone()));
                    let (orchestrator, persona) = match &agent_id {
                        None => (base, None),
                        Some(id) => match scope_chat_agent(&base, settings.as_deref(), id, &conversations_dir, &tx) {
                            Ok((scoped, persona)) => (scoped, Some(persona)),
                            Err(message) => {
                                let _ = tx.send(ServerMessage::ChatError { message, conversation_id: Some(conversation_id), spend_limit_id: None });
                                continue;
                            }
                        },
                    };
                    // `manage_agents` and SSH hosts that need a yes ask this device.
                    let orchestrator = orchestrator.with_approver(Arc::new(approver.clone()));
                    // P84: a member's turn runs in their own space, with only the tools that stay in it.
                    let orchestrator = match &member {
                        Some(member) => member_orchestrator(&orchestrator, member),
                        None => orchestrator,
                    };
                    // A task's conversation (P92) lives with the tasks; the person can go on talking in it.
                    let conversations_dir = conversation_dirs.dir_for(&conversation_id).to_path_buf();
                    let reply_tx = tx.clone();
                    tokio::spawn(async move {
                        let agent = agent_id.as_deref().zip(persona.as_deref()).map(|(id, persona)| TurnAgent { id, persona });
                        let reply = match warden_bootstrap::handle_agent_turn(
                            &orchestrator,
                            &conversations_dir,
                            &conversation_id,
                            &title_seed(&message, &attachments),
                            &message,
                            attachments,
                            agent,
                        )
                        .await
                        {
                            Ok(outcome) => ServerMessage::ChatResponse {
                                content: outcome.content,
                                usage: outcome.usage,
                                attachments: outcome.attachments,
                                conversation_id: Some(conversation_id),
                                fallbacks: outcome.fallbacks.into_iter().map(Into::into).collect(),
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
                Ok(ClientMessage::ResolveApproval { approval_id, approved }) => {
                    approver.resolve(approval_id, approved);
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
                    let vault = member.as_ref().map(|m| m.vault.clone()).unwrap_or_else(|| orchestrator.current().vault().clone());
                    let store = SkillStore::new(vault);
                    if let Some(reply) = handle_skill_request(&store, message) {
                        let _ = tx.send(reply);
                    }
                }
                Ok(ClientMessage::RequestHistory { request_id, limit, conversation_id }) => {
                    // P40 — one small file read, answered inline like the skills requests. Inline
                    // also means a `Chat` sent right after this request can never land in the
                    // reply: that turn is only saved once its (spawned) model call finishes.
                    let _ = tx.send(handle_history_request(&conversation_dirs, request_id, limit, conversation_id));
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
                    let vault = member.as_ref().map(|m| m.vault.clone()).unwrap_or_else(|| orchestrator.current().vault().clone());
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
                    let tasks_dir = tasks_dir.clone();
                    let pairing = PairingStore::new(devices_path.as_ref().clone());
                    let guard = orchestrator.current().spend_guard().cloned();
                    let reply_tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = reply_tx.send(handle_usage_request(&root, tasks_dir.as_deref().map(PathBuf::as_path), &pairing, guard.as_deref(), request_id, tz_offset_minutes));
                    });
                }
                Ok(ClientMessage::ExtendLimit { request_id, limit_id }) => {
                    let guard = orchestrator.current().spend_guard().cloned();
                    let reply_tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = reply_tx.send(handle_extend_limit(guard.as_deref(), request_id, &limit_id));
                    });
                }
                Ok(ClientMessage::RequestSettings { request_id }) => {
                    let access = SettingsAccess { host: settings.as_deref(), shared: &shared_orchestrator, lock: &settings_lock, auth_key: &auth_key, secure };
                    let reply = handle_request_settings(&access, request_id);
                    // P84: a member only sees the agents they can pick.
                    let _ = tx.send(if member.is_some() { member_settings_view(reply) } else { reply });
                }
                Ok(ClientMessage::SaveSettings { request_id, pairing_key, base_version, update }) => {
                    // P78 — rebuilding the orchestrator starts MCP servers and can take seconds.
                    let settings = settings.clone();
                    let shared = shared_orchestrator.clone();
                    let lock = settings_lock.clone();
                    let auth_key = auth_key.clone();
                    let reply_tx = tx.clone();
                    tokio::spawn(async move {
                        let access = SettingsAccess { host: settings.as_deref(), shared: &shared, lock: &lock, auth_key: &auth_key, secure };
                        let _ = reply_tx.send(handle_save_settings(&access, request_id, &pairing_key, &base_version, update).await);
                    });
                }
                Ok(ClientMessage::ListDevices { request_id }) => {
                    let store = PairingStore::new(devices_path.as_ref().clone());
                    let _ = tx.send(handle_list_devices(&store, &device_id, request_id));
                }
                Ok(ClientMessage::SetDeviceStatus { request_id, pairing_key, device_id: target, action }) => {
                    // Off the reader loop: a wrong key waits a second under the settings lock.
                    let store = PairingStore::new(devices_path.as_ref().clone());
                    let lock = settings_lock.clone();
                    let auth_key = auth_key.clone();
                    let you = device_id.clone();
                    let reply_tx = tx.clone();
                    tokio::spawn(async move {
                        let reply = handle_set_device_status(&store, &lock, &auth_key, &you, request_id, &pairing_key, &target, action).await;
                        let _ = reply_tx.send(reply);
                    });
                }
                Ok(ClientMessage::ListApiKeys { request_id }) => {
                    let store = api_keys.as_deref().map(|path| ApiKeyStore::new(path.clone()));
                    let _ = tx.send(handle_list_api_keys(store.as_ref(), request_id));
                }
                Ok(ClientMessage::CreateApiKey { request_id, pairing_key, name, agent_id }) => {
                    spawn_api_key_change(&api_keys, &settings, &settings_lock, &auth_key, &tx, request_id, pairing_key, ApiKeyChange::Create { name, agent_id });
                }
                Ok(ClientMessage::RevokeApiKey { request_id, pairing_key, id }) => {
                    spawn_api_key_change(&api_keys, &settings, &settings_lock, &auth_key, &tx, request_id, pairing_key, ApiKeyChange::Revoke { id });
                }
                Ok(ClientMessage::ModelEvent { request_id, event }) => {
                    if let Some(models) = &node_models {
                        models.deliver(request_id, event);
                    }
                }
                Ok(ClientMessage::ModelDone { request_id }) => {
                    if let Some(models) = &node_models {
                        models.finish(request_id);
                    }
                }
                Ok(ClientMessage::ModelError { request_id, message, transient }) => {
                    if let Some(models) = &node_models {
                        models.fail(request_id, message, transient);
                    }
                }
                Ok(ClientMessage::ListNodes { request_id }) => {
                    let pairing = PairingStore::new(devices_path.as_ref().clone());
                    let _ = tx.send(handle_list_nodes(&nodes, &pairing, settings.as_deref(), request_id));
                }
                Ok(ClientMessage::SetNodeAccess { request_id, pairing_key, device_id: node_id, enabled, agents, require_approval }) => {
                    let (nodes, settings, lock, auth_key, reply_tx) = (nodes.clone(), settings.clone(), settings_lock.clone(), auth_key.clone(), tx.clone());
                    let pairing = PairingStore::new(devices_path.as_ref().clone());
                    tokio::spawn(async move {
                        let access = warden_bootstrap::NodeAccessConfig { id: node_id, enabled, agents, require_approval };
                        let reply = handle_set_node_access(&nodes, &pairing, settings.as_deref(), &lock, &auth_key, request_id, &pairing_key, access).await;
                        let _ = reply_tx.send(reply);
                    });
                }
                Ok(ClientMessage::ListTasks { request_id }) => {
                    let _ = tx.send(handle_list_tasks(tasks.as_ref(), settings.as_deref(), request_id));
                }
                Ok(ClientMessage::SaveTask { request_id, pairing_key, original_id, task }) => {
                    spawn_task_change(&tasks, &settings, &shared_orchestrator, &settings_lock, &auth_key, &tx, request_id, pairing_key, TaskChange::Save { original_id, task });
                }
                Ok(ClientMessage::SetTaskEnabled { request_id, pairing_key, id, enabled }) => {
                    spawn_task_change(&tasks, &settings, &shared_orchestrator, &settings_lock, &auth_key, &tx, request_id, pairing_key, TaskChange::SetEnabled { id, enabled });
                }
                Ok(ClientMessage::DeleteTask { request_id, pairing_key, id }) => {
                    spawn_task_change(&tasks, &settings, &shared_orchestrator, &settings_lock, &auth_key, &tx, request_id, pairing_key, TaskChange::Delete { id });
                }
                Ok(ClientMessage::RunTask { request_id, pairing_key, id }) => {
                    spawn_task_change(&tasks, &settings, &shared_orchestrator, &settings_lock, &auth_key, &tx, request_id, pairing_key, TaskChange::Run { id });
                }
                Ok(ClientMessage::RequestSyncStatus { request_id }) => {
                    // Computes the pending diff over the whole vault: off the reader loop.
                    let sync = sync.clone();
                    let reply_tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = reply_tx.send(handle_sync_status(sync.as_deref(), request_id));
                    });
                }
                Ok(ClientMessage::SyncAction { request_id, pairing_key, action }) => {
                    // A round or a pairing takes seconds; a wrong key waits one under the settings lock.
                    let (sync, settings, shared) = (sync.clone(), settings.clone(), shared_orchestrator.clone());
                    let (lock, auth_key) = (settings_lock.clone(), auth_key.clone());
                    let reply_tx = tx.clone();
                    tokio::spawn(async move {
                        let access = SyncAccess { runner: sync.as_deref(), lock: &lock, auth_key: &auth_key, settings: settings.as_deref(), shared: &shared };
                        let _ = reply_tx.send(handle_sync_action(&access, request_id, &pairing_key, action).await);
                    });
                }
                Ok(message @ (ClientMessage::ListConversations { .. } | ClientMessage::RenameConversation { .. } | ClientMessage::DeleteConversation { .. })) => {
                    // P78 — small file I/O, answered inline like `RequestHistory`.
                    if let Some(reply) = handle_conversation_request(&conversation_dirs, message) {
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
    if nodes.disconnect(&device_id, &tool_channel) {
        refresh_node_mcp_tools(&shared_orchestrator, node_tools.as_ref());
    }
    if let Some(models) = &node_models {
        models.close(&device_name);
    }
    // Calls still waiting on this device (a node's long command) fail now instead of at their timeout.
    tool_channel.close();
    // Nobody is left to answer: open approvals count as a no right away.
    approver.close();
    // `tool_channel`, the approver and this connection's own `Orchestrator` (its `RemoteTool`s) hold
    // `tx` clones too — dropped here so the writer task actually ends once in-flight `Chat` tasks
    // finish, instead of waiting on a sender that lives as long as this function.
    drop((tx, tool_channel, orchestrator, approver));
    writer_task.await.ok();

    Ok(())
}

/// A `Chat` turn's orchestrator speaking as the configured agent `agent_id` (P46), and its persona.
/// The config is read fresh from the hub's settings file, so an agent created or edited a moment ago
/// (web settings, `manage_agents`, the desktop) is what answers. `message_agent` writes into this
/// device's conversations and tells it through `ConversationsChanged` — through a weak sender, so an
/// agent still answering after the device left doesn't keep the connection's writer alive.
fn scope_chat_agent(
    base: &Orchestrator,
    settings: Option<&dyn SettingsHost>,
    agent_id: &str,
    conversations_dir: &Path,
    tx: &mpsc::UnboundedSender<ServerMessage>,
) -> Result<(Orchestrator, String), String> {
    let host = settings.ok_or_else(|| "this hub has no settings file, so it has no agents".to_string())?;
    let path = host.config_path();
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    let notify_tx = tx.downgrade();
    let extras = AgentExtras {
        conversations_dir: Some(conversations_dir.to_path_buf()),
        on_conversation_changed: Some(Arc::new(move |conversation_id: &str| {
            if let Some(tx) = notify_tx.upgrade() {
                let _ = tx.send(ServerMessage::ConversationsChanged { conversation_id: conversation_id.to_string() });
            }
        })),
    };
    let scoped = scope_to_agent(base, &config, Some(&path), agent_id, extras).ok_or_else(|| format!("agent '{agent_id}' not found"))?;
    let mut orchestrator = scoped.orchestrator;
    if let Some(provider_id) = &scoped.provider_id {
        let model = build_model_for(&config, provider_id, None).map_err(|e| format!("agent '{agent_id}' can't use its model '{provider_id}': {e:#}"))?;
        orchestrator = orchestrator.with_model(model);
    }
    Ok((orchestrator, scoped.persona))
}

/// One connection's view of the hub's orchestrator: the shared one, plus a `RemoteTool` for every
/// tool this device advertised in `Hello` (Fase 7.4). Built again only when a settings save has
/// swapped the shared orchestrator (P78), so a turn costs a pointer comparison, not a rebuild.
struct ConnectionOrchestrator {
    shared: SharedOrchestrator,
    tools: Vec<ToolSpec>,
    channel: RemoteToolChannel,
    device_id: String,
    /// The shared orchestrator this was built from, and what was built.
    built: Option<(Arc<Orchestrator>, Arc<Orchestrator>)>,
}

impl ConnectionOrchestrator {
    fn new(shared: SharedOrchestrator, tools: Vec<ToolSpec>, channel: RemoteToolChannel, device_id: String) -> Self {
        Self { shared, tools, channel, device_id, built: None }
    }

    fn current(&mut self) -> Arc<Orchestrator> {
        let base = self.shared.current();
        if self.tools.is_empty() {
            return base;
        }
        if let Some((from, built)) = &self.built {
            if Arc::ptr_eq(from, &base) {
                return built.clone();
            }
        }
        let built = Arc::new(self.with_device_tools(&base));
        self.built = Some((base, built.clone()));
        built
    }

    // P42: a client's advertised name can collide with the shared Orchestrator's own tools (vault,
    // shell, SSH, MCP servers) — the original real case was the phone's first `list_files`/
    // `read_file` colliding with the vault's own tools of the same name, which broke the next model
    // call with a provider-side "duplicate function" error rather than anything clear from Warden.
    // Deduped the same way `warden-bootstrap::register_mcp_tools` dedupes an MCP server's tools
    // (P46) — only renamed on a real collision, namespaced by this device's id, via the shared
    // `warden_core::tool::dedupe_tool_name`/`rename_tool`. `RemoteTool::call` sends the request
    // using its own internal spec, never what this wrapper reports, so the client is never told
    // about the rename — it keeps answering to the name it always advertised.
    fn with_device_tools(&self, base: &Orchestrator) -> Orchestrator {
        let mut per_connection = base.clone();
        for spec in &self.tools {
            let original = spec.name.clone();
            let existing: Vec<String> = per_connection.tools().iter().map(|t| t.spec().name).collect();
            let resolved = warden_core::tool::dedupe_tool_name(&existing, &self.device_id, &original);
            let remote = Arc::new(RemoteTool::new(spec.clone(), self.channel.clone(), REMOTE_TOOL_TIMEOUT));
            if resolved == original {
                per_connection.register_tool(remote);
            } else {
                eprintln!(
                    "warden-server: {}'s tool '{original}' collides with an already-registered tool — renamed to '{resolved}'\n",
                    self.device_id
                );
                per_connection.register_tool(warden_core::tool::rename_tool(remote, resolved));
            }
        }
        per_connection
    }
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
