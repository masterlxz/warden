//! P102 — this process as a **client** of another hub: the desktop's native interface talking to a hub on the VPS the
//! way the web page does, instead of to its own engine. One task owns the WebSocket (`ServerConnection` has no split),
//! and everything else talks to it through a [`RemoteHandle`]:
//!
//! - **Requests** (`ListConversations`, `RequestHistory`, `ListProjects`, `ListDirs`...) carry a `request_id` the hub
//!   echoes; each is matched to the call that made it.
//! - **A turn** (`Chat`) has no request id: its answer is matched by `conversation_id`. Turns of different conversations
//!   run side by side; a second turn in a conversation that is still answering waits its turn, because the hub doesn't
//!   guard two turns of one conversation racing on its file.
//! - **What the hub pushes** (`ChatEvent`, `ApprovalRequest`/`ApprovalCancelled`, `ConversationsChanged`) goes out through
//!   a [`RemoteSink`], and so does the connection's state. The desktop turns them into the same Tauri events the local
//!   engine already emits, so the screens don't change.
//! - **The connection comes back by itself** (1 s doubling to 60 s), with the device token the hub issued: the pairing key
//!   or the member's password is only ever used for the first sign-in, and neither is kept. A hub that turns the device
//!   away (a wrong key, a revoked token) ends it: retrying can't fix that.
//!
//! Its own identity (`remote_hub.json`, one device id and token per hub) is not the node's (`node.json`): the same
//! device would otherwise be a node and a client of one hub at once, and pairing again resets its status.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};
use warden_server_protocol::protocol::{ChatEventDto, ClientMessage, ServerMessage, UserInfoDto};
use warden_server_protocol::tls::default_client_config;
use warden_server_protocol::{AuthRejected, ServerConnection};

const HEARTBEAT: Duration = Duration::from_secs(20);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How long to wait for the hub to accept a connection and answer the Hello.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// What a turn may take: a model with tools can run for minutes.
pub const TURN_TIMEOUT: Duration = Duration::from_secs(600);
/// What an ordinary request (a list, a save) may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

// ---- who this client is to each hub ----

/// One hub's view of this client: the device id it knows, and the token it issued for it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RemoteIdentity {
    pub device_id: String,
    #[serde(default)]
    pub device_token: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct IdentityFile {
    #[serde(default)]
    hubs: BTreeMap<String, RemoteIdentity>,
}

/// The identities of one `config.toml`, in `remote_hub.json` beside it, keyed by the saved hub's id. Read again on every
/// call, and written atomically (owner-readable only): the token is what lets this computer speak to that hub.
pub struct RemoteIdentities {
    path: PathBuf,
}

impl RemoteIdentities {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The file that goes with the `config.toml` at `config_path`.
    pub fn beside(config_path: &Path) -> Self {
        Self::new(config_path.with_file_name("remote_hub.json"))
    }

    fn read(&self) -> anyhow::Result<IdentityFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(IdentityFile::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    fn write(&self, file: &IdentityFile) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(file)?).with_context(|| format!("writing {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &self.path).with_context(|| format!("replacing {}", self.path.display()))
    }

    /// This client's identity for `hub_id`: the saved one, or a new device id (`desktop-<name>-<8 hex>`) the first time.
    pub fn load_or_create(&self, hub_id: &str, device_name: &str) -> anyhow::Result<RemoteIdentity> {
        let mut file = self.read()?;
        if let Some(identity) = file.hubs.get(hub_id) {
            return Ok(identity.clone());
        }
        use sha2::{Digest, Sha256};
        let seed = format!("{device_name}-{hub_id}-{:?}-{}", std::time::SystemTime::now(), std::process::id());
        let suffix: String = Sha256::digest(seed.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect();
        let slug: String = device_name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).take(24).collect();
        let identity = RemoteIdentity { device_id: format!("desktop-{slug}-{suffix}"), device_token: None };
        file.hubs.insert(hub_id.to_string(), identity.clone());
        self.write(&file)?;
        Ok(identity)
    }

    /// Keeps the token the hub issued for `hub_id`.
    pub fn set_token(&self, hub_id: &str, token: &str) -> anyhow::Result<()> {
        let mut file = self.read()?;
        let Some(identity) = file.hubs.get_mut(hub_id) else {
            anyhow::bail!("there is no identity for hub '{hub_id}' yet");
        };
        identity.device_token = Some(token.to_string());
        self.write(&file)
    }

    /// Forgets the token for `hub_id` (signing out, or the hub turned it away); the device id stays.
    pub fn clear_token(&self, hub_id: &str) -> anyhow::Result<()> {
        let mut file = self.read()?;
        if let Some(identity) = file.hubs.get_mut(hub_id) {
            if identity.device_token.take().is_some() {
                self.write(&file)?;
            }
        }
        Ok(())
    }

    /// Forgets everything about `hub_id` (the hub was removed from the list).
    pub fn forget(&self, hub_id: &str) -> anyhow::Result<()> {
        let mut file = self.read()?;
        if file.hubs.remove(hub_id).is_some() {
            self.write(&file)?;
        }
        Ok(())
    }
}

// ---- what is told to whoever shows it ----

/// How the first sign-in is made. Used once: afterwards only the token the hub issues is kept.
#[derive(Clone)]
pub enum RemoteCredential {
    /// The hub's pairing key: signs in as its owner.
    PairingKey(String),
    /// A member of the workspace.
    Member { username: String, password: String },
}

/// Everything one connection needs. Not `Debug` on purpose: it holds a key or a password.
#[derive(Clone)]
pub struct RemoteConfig {
    /// `ws://host:port` or `wss://host:port`.
    pub url: String,
    pub device_id: String,
    pub device_name: String,
    /// For the first sign-in; wins over `token` when both are there (the person signed in again).
    pub credential: Option<RemoteCredential>,
    pub token: Option<String>,
}

/// Where the connection to the hub stands.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum RemoteState {
    Connecting,
    /// `user` is `None` for the owner; for a member it says whose data this is, and whether it is `locked` (encrypted, and
    /// only a password sign-in opens it again after the hub restarted).
    Connected { user: Option<Box<UserInfoDto>> },
    #[serde(rename_all = "camelCase")]
    Retrying { error: String, in_secs: u64 },
    /// Ended: the hub turned this device away, or it was disconnected (`error` is `None` then).
    Stopped { error: Option<String> },
}

/// What the hub pushes, and the connection's own news.
#[derive(Clone, Debug)]
pub enum RemoteEvent {
    State(RemoteState),
    /// The hub issued a token for this device: keep it, and it is all the next connection needs.
    NewToken(String),
    ChatEvent { conversation_id: String, event: ChatEventDto },
    /// Ids here are the hub's, per connection.
    Approval { approval_id: u64, target: String, action: String, detail: String, always: Option<String>, category: Option<String> },
    ApprovalCancelled { approval_id: u64 },
    ConversationsChanged { conversation_id: String },
    /// The owner changed what this member may do with the organization of the agents (P120): `none`, `view` or `edit`.
    OrgAccessChanged { access: String },
}

/// Where [`RemoteEvent`]s go. The desktop emits Tauri events; a test collects them.
pub trait RemoteSink: Send + Sync {
    fn emit(&self, event: RemoteEvent);
}

// ---- the task that owns the connection ----

type Reply = oneshot::Sender<Result<ServerMessage, String>>;

enum Command {
    Request { id: u64, message: ClientMessage, reply: Reply },
    Forget(u64),
    Turn { conversation_id: String, message: ClientMessage, reply: Reply },
    Send(ClientMessage),
    Stop,
}

/// A way to talk to the connection. Cheap to clone; the connection ends with [`stop`](Self::stop), or when every handle
/// is gone.
#[derive(Clone)]
pub struct RemoteHandle {
    commands: mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
    state: watch::Receiver<RemoteState>,
}

impl RemoteHandle {
    /// Starts connecting. Needs a Tokio runtime.
    pub fn start(config: RemoteConfig, sink: Arc<dyn RemoteSink>) -> Self {
        let (commands, rx) = mpsc::unbounded_channel();
        let (state_tx, state) = watch::channel(RemoteState::Connecting);
        tokio::spawn(run(config, sink, rx, state_tx));
        Self { commands, next_id: Arc::new(AtomicU64::new(0)), state }
    }

    pub fn state(&self) -> RemoteState {
        self.state.borrow().clone()
    }

    /// Sends a request and waits for the reply that carries its `request_id`. `build` gets that id and makes the
    /// message (so a caller holding JSON can put it in). Hub-side refusals (`ConversationError`...) are replies too:
    /// `Err` is only for a connection that is down, a timeout, or a message that couldn't be built.
    pub async fn request(&self, build: impl FnOnce(u64) -> Result<ClientMessage, String>, timeout: Duration) -> Result<ServerMessage, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let message = build(id)?;
        let (reply, answer) = oneshot::channel();
        self.commands.send(Command::Request { id, message, reply }).map_err(|_| "the connection to the hub has ended".to_string())?;
        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("the connection to the hub dropped".to_string()),
            Err(_) => {
                let _ = self.commands.send(Command::Forget(id));
                Err(format!("the hub did not answer within {}s", timeout.as_secs()))
            }
        }
    }

    /// Sends a `Chat` and waits for its `ChatResponse` or `ChatError`. A turn of the same conversation that is still
    /// answering is waited for first.
    pub async fn turn(&self, conversation_id: &str, message: ClientMessage, timeout: Duration) -> Result<ServerMessage, String> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(Command::Turn { conversation_id: conversation_id.to_string(), message, reply }).map_err(|_| "the connection to the hub has ended".to_string())?;
        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("the connection to the hub dropped".to_string()),
            Err(_) => Err(format!("the turn took longer than {}s", timeout.as_secs())),
        }
    }

    /// Sends a message that has no reply (`CancelTurn`, `SetCodeMode`, `ResolveApproval`). Lost if the connection is down.
    pub fn send(&self, message: ClientMessage) {
        let _ = self.commands.send(Command::Send(message));
    }

    /// Ends the connection (and with it any reconnecting).
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }
}

#[derive(Debug, PartialEq)]
enum End {
    Stop,
    Closed,
}

struct Outcome {
    /// The hub accepted the sign-in at least once in this run.
    signed_in: bool,
    token: Option<String>,
    end: anyhow::Result<End>,
}

async fn run(mut config: RemoteConfig, sink: Arc<dyn RemoteSink>, mut rx: mpsc::UnboundedReceiver<Command>, state: watch::Sender<RemoteState>) {
    let set = |next: RemoteState| {
        state.send_replace(next.clone());
        sink.emit(RemoteEvent::State(next));
    };
    if config.token.is_none() && config.credential.is_none() {
        set(RemoteState::Stopped { error: Some("not signed in to this hub: give its pairing key or a username and password".to_string()) });
        return;
    }
    let mut backoff = Duration::from_secs(1);
    loop {
        set(RemoteState::Connecting);
        let outcome = serve_once(&config, &mut rx, &sink, &set).await;
        if outcome.signed_in {
            backoff = Duration::from_secs(1);
            // The key or password did its one job: from here on the token signs in.
            config.credential = None;
            if let Some(token) = outcome.token {
                config.token = Some(token);
            }
        }
        let error = match outcome.end {
            Ok(End::Stop) => {
                set(RemoteState::Stopped { error: None });
                return;
            }
            Ok(End::Closed) => "the hub closed the connection".to_string(),
            Err(err) if err.downcast_ref::<AuthRejected>().is_some() => {
                set(RemoteState::Stopped { error: Some(format!("{err:#}")) });
                return;
            }
            Err(err) => format!("{err:#}"),
        };
        set(RemoteState::Retrying { error, in_secs: backoff.as_secs() });
        // While waiting, whatever is asked is told the hub isn't there instead of piling up.
        let deadline = tokio::time::Instant::now() + backoff;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                command = rx.recv() => match command {
                    None | Some(Command::Stop) => {
                        set(RemoteState::Stopped { error: None });
                        return;
                    }
                    Some(Command::Request { reply, .. }) | Some(Command::Turn { reply, .. }) => {
                        let _ = reply.send(Err("not connected to the hub right now; it is being retried".to_string()));
                    }
                    Some(Command::Forget(_)) | Some(Command::Send(_)) => {}
                },
            }
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn connect(config: &RemoteConfig) -> anyhow::Result<(ServerConnection, Option<String>, Option<UserInfoDto>)> {
    let tls = default_client_config();
    match (&config.credential, &config.token) {
        (Some(RemoteCredential::PairingKey(key)), _) => {
            let (conn, token) = ServerConnection::handshake_with_tls(&config.url, &config.device_id, &config.device_name, key, None, Vec::new(), tls).await?;
            // The pairing key is the owner's: the hub says no user.
            Ok((conn, token, None))
        }
        (Some(RemoteCredential::Member { username, password }), _) => {
            // This client doesn't show a recovery code, so it must never make the hub create a data key at this sign-in.
            ServerConnection::handshake_as_member_showing(&config.url, &config.device_id, &config.device_name, username, password, None, tls, false).await
        }
        (None, Some(token)) => ServerConnection::handshake_with_token(&config.url, &config.device_id, &config.device_name, token, tls).await,
        (None, None) => anyhow::bail!("not signed in to this hub"),
    }
}

async fn serve_once(config: &RemoteConfig, rx: &mut mpsc::UnboundedReceiver<Command>, sink: &Arc<dyn RemoteSink>, set: &(dyn Fn(RemoteState) + Sync)) -> Outcome {
    let mut outcome = Outcome { signed_in: false, token: None, end: Ok(End::Closed) };
    let (mut conn, issued, user) = match tokio::time::timeout(CONNECT_TIMEOUT, connect(config)).await {
        Ok(Ok(connected)) => connected,
        Ok(Err(err)) => {
            outcome.end = Err(err);
            return outcome;
        }
        Err(_) => {
            outcome.end = Err(anyhow::anyhow!("the hub did not answer within {}s", CONNECT_TIMEOUT.as_secs()));
            return outcome;
        }
    };
    outcome.signed_in = true;
    outcome.token = issued.clone();
    if let Some(token) = issued {
        sink.emit(RemoteEvent::NewToken(token));
    }
    set(RemoteState::Connected { user: user.map(Box::new) });

    let mut core = Core::default();
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.tick().await;
    let mut nonce = 0u64;
    let end = loop {
        tokio::select! {
            incoming = conn.recv() => match incoming {
                Ok(Some(message)) => {
                    if let Err(err) = core.on_server(message, &mut conn, sink).await {
                        break Err(err);
                    }
                }
                Ok(None) => break Ok(End::Closed),
                // A message this build doesn't know (a newer hub): the socket is fine, only that frame is skipped.
                Err(err) if err.downcast_ref::<serde_json::Error>().is_some() => {
                    eprintln!("remote hub: skipping a message this version doesn't understand: {err}");
                }
                Err(err) => break Err(err),
            },
            command = rx.recv() => match command {
                None | Some(Command::Stop) => break Ok(End::Stop),
                Some(command) => {
                    if let Err(err) = core.on_command(command, &mut conn).await {
                        break Err(err);
                    }
                }
            },
            _ = heartbeat.tick() => {
                nonce += 1;
                if let Err(err) = conn.ping(nonce).await {
                    break Err(err);
                }
            }
        }
    };
    core.finish(sink);
    outcome.end = end;
    outcome
}

/// What one connection has in flight. Dropped with the connection: everything waiting is told, and every approval the
/// hub was still waiting on is closed (the hub counts those as a no when the socket goes).
#[derive(Default)]
struct Core {
    pending: HashMap<u64, Reply>,
    turns: HashMap<String, InFlight>,
    approvals: HashSet<u64>,
}

struct InFlight {
    reply: Reply,
    /// Later turns of the same conversation, in the order they were asked.
    queue: VecDeque<(ClientMessage, Reply)>,
}

impl Core {
    async fn on_command(&mut self, command: Command, conn: &mut ServerConnection) -> anyhow::Result<()> {
        match command {
            Command::Request { id, message, reply } => match conn.send(&message).await {
                Ok(()) => {
                    self.pending.insert(id, reply);
                }
                Err(err) => {
                    let _ = reply.send(Err(format!("{err:#}")));
                    return Err(err);
                }
            },
            Command::Forget(id) => {
                self.pending.remove(&id);
            }
            Command::Turn { conversation_id, message, reply } => {
                if let Some(flight) = self.turns.get_mut(&conversation_id) {
                    flight.queue.push_back((message, reply));
                } else {
                    match conn.send(&message).await {
                        Ok(()) => {
                            self.turns.insert(conversation_id, InFlight { reply, queue: VecDeque::new() });
                        }
                        Err(err) => {
                            let _ = reply.send(Err(format!("{err:#}")));
                            return Err(err);
                        }
                    }
                }
            }
            Command::Send(message) => {
                if let ClientMessage::ResolveApproval { approval_id, .. } = &message {
                    self.approvals.remove(approval_id);
                }
                conn.send(&message).await?;
            }
            Command::Stop => {}
        }
        Ok(())
    }

    async fn on_server(&mut self, message: ServerMessage, conn: &mut ServerConnection, sink: &Arc<dyn RemoteSink>) -> anyhow::Result<()> {
        match message {
            ServerMessage::Pong { .. } => {}
            ServerMessage::AuthError { reason } => return Err(AuthRejected { reason }.into()),
            ServerMessage::ChatEvent { conversation_id, event } => sink.emit(RemoteEvent::ChatEvent { conversation_id, event }),
            ServerMessage::ApprovalRequest { approval_id, target, action, detail, always, category } => {
                self.approvals.insert(approval_id);
                sink.emit(RemoteEvent::Approval { approval_id, target, action, detail, always, category });
            }
            ServerMessage::ApprovalCancelled { approval_id } => {
                self.approvals.remove(&approval_id);
                sink.emit(RemoteEvent::ApprovalCancelled { approval_id });
            }
            ServerMessage::ConversationsChanged { conversation_id } => sink.emit(RemoteEvent::ConversationsChanged { conversation_id }),
            ServerMessage::OrgAccessChanged { access } => sink.emit(RemoteEvent::OrgAccessChanged { access }),
            ServerMessage::ChatResponse { conversation_id: ref id, .. } | ServerMessage::ChatError { conversation_id: ref id, .. } => {
                let key = id.clone().filter(|c| self.turns.contains_key(c)).or_else(|| if self.turns.len() == 1 { self.turns.keys().next().cloned() } else { None });
                let Some(key) = key else {
                    eprintln!("remote hub: an answer for a turn nobody is waiting on ({id:?})");
                    return Ok(());
                };
                let flight = self.turns.remove(&key).expect("the key was just found");
                let _ = flight.reply.send(Ok(message));
                let mut queue = flight.queue;
                if let Some((next, reply)) = queue.pop_front() {
                    match conn.send(&next).await {
                        Ok(()) => {
                            self.turns.insert(key, InFlight { reply, queue });
                        }
                        Err(err) => {
                            let _ = reply.send(Err(format!("{err:#}")));
                            for (_, waiting) in queue {
                                let _ = waiting.send(Err("the connection to the hub dropped".to_string()));
                            }
                            return Err(err);
                        }
                    }
                }
            }
            other => {
                // Every other reply to a request carries its `requestId`; find it without naming each variant.
                let id = serde_json::to_value(&other).ok().and_then(|v| v.get("requestId").and_then(|id| id.as_u64()));
                match id.and_then(|id| self.pending.remove(&id)) {
                    Some(reply) => {
                        let _ = reply.send(Ok(other));
                    }
                    None => eprintln!("remote hub: a message nobody asked for was skipped"),
                }
            }
        }
        Ok(())
    }

    /// The connection is over: tell everyone still waiting, and close every approval the hub was waiting on.
    fn finish(&mut self, sink: &Arc<dyn RemoteSink>) {
        const GONE: &str = "the connection to the hub dropped";
        for (_, reply) in self.pending.drain() {
            let _ = reply.send(Err(GONE.to_string()));
        }
        for (_, flight) in self.turns.drain() {
            let _ = flight.reply.send(Err(GONE.to_string()));
            for (_, waiting) in flight.queue {
                let _ = waiting.send(Err(GONE.to_string()));
            }
        }
        for approval_id in self.approvals.drain() {
            sink.emit(RemoteEvent::ApprovalCancelled { approval_id });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn temp_file() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("warden-remote-identities-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("remote_hub.json")
    }

    #[test]
    fn each_hub_gets_its_own_device_id_and_token_and_they_survive_a_reload() {
        let path = temp_file();
        let store = RemoteIdentities::new(path.clone());
        let home = store.load_or_create("hub-1", "My Desktop!").unwrap();
        let vps = store.load_or_create("hub-2", "My Desktop!").unwrap();
        assert!(home.device_id.starts_with("desktop-my-desktop--"), "{}", home.device_id);
        assert_ne!(home.device_id, vps.device_id, "one device id per hub");
        assert_eq!(home.device_token, None);
        assert_eq!(store.load_or_create("hub-1", "another name").unwrap(), home, "the saved one wins over a new name");

        store.set_token("hub-1", "tok-1").unwrap();
        let again = RemoteIdentities::new(path.clone());
        assert_eq!(again.load_or_create("hub-1", "x").unwrap().device_token.as_deref(), Some("tok-1"));
        assert_eq!(again.load_or_create("hub-2", "x").unwrap().device_token, None, "the other hub's token is its own");
        assert!(store.set_token("hub-nope", "t").is_err());

        store.clear_token("hub-1").unwrap();
        let cleared = store.load_or_create("hub-1", "x").unwrap();
        assert_eq!((cleared.device_id.as_str(), cleared.device_token.as_deref()), (home.device_id.as_str(), None), "the id stays, the token goes");
        store.forget("hub-2").unwrap();
        assert_ne!(store.load_or_create("hub-2", "x").unwrap().device_id, vps.device_id, "forgotten, so a new id");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_file_holds_a_device_id_and_a_token_and_is_the_owners() {
        let path = temp_file();
        let store = RemoteIdentities::new(path.clone());
        store.load_or_create("hub-1", "Desktop").unwrap();
        store.set_token("hub-1", "tok-1").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut keys: Vec<&str> = json["hubs"]["hub-1"].as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["device_id", "device_token"], "no key or password field exists: {text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_broken_file_is_an_error_and_is_left_alone() {
        let path = temp_file();
        std::fs::write(&path, "{ not json").unwrap();
        let store = RemoteIdentities::new(path.clone());
        assert!(store.load_or_create("hub-1", "Desktop").is_err());
        assert!(store.set_token("hub-1", "t").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_state_is_written_the_way_the_screens_read_it() {
        assert_eq!(serde_json::to_string(&RemoteState::Connecting).unwrap(), r#"{"state":"connecting"}"#);
        assert_eq!(serde_json::to_string(&RemoteState::Retrying { error: "x".into(), in_secs: 4 }).unwrap(), r#"{"state":"retrying","error":"x","inSecs":4}"#);
        assert_eq!(serde_json::to_string(&RemoteState::Stopped { error: None }).unwrap(), r#"{"state":"stopped","error":null}"#);
        assert_eq!(serde_json::to_string(&RemoteState::Connected { user: None }).unwrap(), r#"{"state":"connected","user":null}"#);
    }
}
