//! P102 — the desktop as a client of a hub (`warden_server::remote_client`), against a real `Server` in process: sign in
//! by key, come back with the token, keep turns of one conversation in order while others run beside it, fail what is
//! waiting when the link drops, reconnect by itself, and carry an approval from the hub to the person and back.
//! A TCP proxy stands between the client and the hub where a test has to cut the link without stopping the hub.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use warden_bootstrap::{load_config_from_path, save_config, AgentConfig, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::remote_client::{RemoteConfig, RemoteCredential, RemoteEvent, RemoteHandle, RemoteSink, RemoteState};
use warden_server::{ClientMessage, PairingStore, Server, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::HistoryRole;

const KEY: &str = "test-key";
const ANA_PASSWORD: &str = "anas-own-pass-123";
const SLOW: Duration = Duration::from_millis(700);

/// "SLOW" waits, "VERYSLOW" waits long, "CREATE" asks `manage_agents` for an agent (which asks the person), a tool result
/// is echoed, anything else is `echo:<message>`.
struct Scripted;

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let last = messages.last().unwrap();
        let reply = |content: String| Ok(response_stream(Response { content, tool_calls: Vec::new(), usage: None }));
        if last.role == Role::Tool {
            return reply(format!("tool said: {}", last.content));
        }
        if last.content.contains("VERYSLOW") {
            tokio::time::sleep(Duration::from_secs(8)).await;
        } else if last.content.contains("SLOW") {
            tokio::time::sleep(SLOW).await;
        }
        if last.content.contains("CREATE") {
            return Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "manage_agents".into(),
                    arguments: json!({ "action": "create", "id": "critic", "persona": "You critique." }),
                    thought_signature: None,
                }],
                usage: None,
            }));
        }
        reply(format!("echo:{}", last.content))
    }
}

struct TestHost {
    path: PathBuf,
}

#[async_trait]
impl SettingsHost for TestHost {
    fn config_path(&self) -> PathBuf {
        self.path.clone()
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        anyhow::bail!("not used by these tests")
    }
}

struct Hub {
    addr: std::net::SocketAddr,
    config_path: PathBuf,
    devices_path: PathBuf,
}

async fn spin_up() -> Hub {
    let dir = std::env::temp_dir().join(format!("warden-server-remote-client-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    let chief = AgentConfig {
        id: "chief".into(),
        persona: "You are the chief.".into(),
        provider_id: None,
        can_delegate_to_agents: false,
        can_manage_agents: true,
        can_message_agents: false,
        can_manage_tasks: false,
        allowed_tools: None,
        autonomy: warden_bootstrap::default_autonomy(),
        approval_required: Vec::new(),
        role: None,
        reports_to: None,
        owner: None,
        shared_with: Vec::new(),
        delegation_models: Vec::new(),
        can_start_tasks: true,
        can_create_workers: true,
    };
    let mut config = FileConfig { agents: vec![chief], ..FileConfig::default() };
    // A member with a password of her own already (no provisional one to change first).
    warden_bootstrap::users::add_user(&mut config, "ana", "Ana", "provisional-pass-1").unwrap();
    config.users[0].password_hash = warden_bootstrap::users::hash_password(ANA_PASSWORD).unwrap();
    config.users[0].must_change_password = false;
    save_config(&config_path, &config).unwrap();
    let devices_path = dir.join("devices.json");
    let orchestrator = Orchestrator::new(Arc::new(Scripted), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), devices_path.clone())
        .await
        .unwrap()
        .with_revocation_check_interval(Duration::from_millis(100))
        .with_users_dir(dir.join("users"))
        .with_settings(Arc::new(TestHost { path: config_path.clone() }));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { addr, config_path, devices_path }
}

/// A TCP forwarder in front of the hub that can cut every connection it carries while still accepting new ones — a
/// link that drops, not a hub that went away.
struct Proxy {
    addr: std::net::SocketAddr,
    links: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Proxy {
    async fn to(target: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let links: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        let kept = links.clone();
        tokio::spawn(async move {
            while let Ok((mut from, _)) = listener.accept().await {
                let link = tokio::spawn(async move {
                    if let Ok(mut to) = TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut from, &mut to).await;
                    }
                });
                kept.lock().unwrap().push(link);
            }
        });
        Self { addr, links }
    }

    fn cut(&self) {
        for link in self.links.lock().unwrap().drain(..) {
            link.abort();
        }
    }
}

#[derive(Default)]
struct Collector(Mutex<Vec<RemoteEvent>>);

impl RemoteSink for Collector {
    fn emit(&self, event: RemoteEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Collector {
    fn events(&self) -> Vec<RemoteEvent> {
        self.0.lock().unwrap().clone()
    }

    fn tokens(&self) -> Vec<String> {
        self.events().into_iter().filter_map(|e| if let RemoteEvent::NewToken(t) = e { Some(t) } else { None }).collect()
    }

    fn states(&self) -> Vec<RemoteState> {
        self.events().into_iter().filter_map(|e| if let RemoteEvent::State(s) = e { Some(s) } else { None }).collect()
    }
}

fn config(addr: std::net::SocketAddr, device: &str, credential: Option<RemoteCredential>, token: Option<String>) -> RemoteConfig {
    RemoteConfig { url: format!("ws://{addr}"), device_id: device.into(), device_name: "Test Desktop".into(), credential, token }
}

fn connect(addr: std::net::SocketAddr, device: &str, credential: Option<RemoteCredential>, token: Option<String>) -> (RemoteHandle, Arc<Collector>) {
    let sink = Arc::new(Collector::default());
    (RemoteHandle::start(config(addr, device, credential, token), sink.clone()), sink)
}

fn owner_key() -> Option<RemoteCredential> {
    Some(RemoteCredential::PairingKey(KEY.into()))
}

/// Waits (up to 15 s) for `check` to hold, polling.
async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn connected(handle: &RemoteHandle) {
    until("the connection to come up", || matches!(handle.state(), RemoteState::Connected { .. })).await;
}

fn chat(message: &str, conversation: &str, agent: Option<&str>) -> ClientMessage {
    ClientMessage::Chat { message: message.into(), conversation_id: Some(conversation.into()), attachments: Vec::new(), agent_id: agent.map(str::to_string), project_id: None, workdir: None, thread_of: None }
}

async fn list(handle: &RemoteHandle) -> Vec<String> {
    match handle.request(|request_id| Ok(ClientMessage::ListConversations { request_id }), Duration::from_secs(10)).await.unwrap() {
        ServerMessage::ConversationList { conversations, .. } => conversations.into_iter().map(|c| c.id).collect(),
        other => panic!("expected ConversationList, got {other:?}"),
    }
}

async fn history(handle: &RemoteHandle, conversation: &str) -> Vec<(HistoryRole, String)> {
    let conversation = conversation.to_string();
    match handle.request(move |request_id| Ok(ClientMessage::RequestHistory { request_id, limit: None, conversation_id: Some(conversation) }), Duration::from_secs(10)).await.unwrap() {
        ServerMessage::History { messages, .. } => messages.into_iter().map(|m| (m.role, m.content)).collect(),
        other => panic!("expected History, got {other:?}"),
    }
}

#[tokio::test]
async fn the_owner_signs_in_with_the_key_lists_asks_and_reads_the_history() {
    let hub = spin_up().await;
    let (handle, sink) = connect(hub.addr, "desktop-1", owner_key(), None);
    connected(&handle).await;
    assert!(matches!(handle.state(), RemoteState::Connected { user: None }), "the pairing key is the owner's: no user");
    assert_eq!(sink.tokens().len(), 1, "the hub issued a token for this device");
    assert_eq!(sink.states().first(), Some(&RemoteState::Connecting));

    assert!(list(&handle).await.is_empty());
    let answer = handle.turn("c1", chat("hello", "c1", None), Duration::from_secs(10)).await.unwrap();
    assert!(matches!(&answer, ServerMessage::ChatResponse { content, conversation_id, .. } if content == "echo:hello" && conversation_id.as_deref() == Some("c1")), "{answer:?}");
    assert_eq!(list(&handle).await, vec!["c1"]);
    assert_eq!(history(&handle, "c1").await, vec![(HistoryRole::User, "hello".to_string()), (HistoryRole::Assistant, "echo:hello".to_string())]);
    handle.stop();
    until("stopped", || sink.states().last() == Some(&RemoteState::Stopped { error: None })).await;
}

#[tokio::test]
async fn the_token_signs_in_without_the_key_and_a_wrong_key_or_nothing_stops_it() {
    let hub = spin_up().await;
    let (first, sink) = connect(hub.addr, "desktop-1", owner_key(), None);
    connected(&first).await;
    let token = sink.tokens().remove(0);
    first.stop();

    let (again, again_sink) = connect(hub.addr, "desktop-1", None, Some(token));
    connected(&again).await;
    assert!(again_sink.tokens().is_empty(), "a token sign-in issues no new token");
    assert!(list(&again).await.is_empty());
    again.stop();

    let (wrong, _) = connect(hub.addr, "desktop-2", Some(RemoteCredential::PairingKey("not-the-key".into())), None);
    until("a wrong key to end it", || matches!(wrong.state(), RemoteState::Stopped { error: Some(_) })).await;
    let RemoteState::Stopped { error: Some(error) } = wrong.state() else { unreachable!() };
    assert!(error.contains("authentication rejected"), "{error}");

    let (nothing, _) = connect(hub.addr, "desktop-3", None, None);
    until("no credential to end it", || matches!(nothing.state(), RemoteState::Stopped { error: Some(_) })).await;
}

fn ana() -> Option<RemoteCredential> {
    Some(RemoteCredential::Member { username: "ana".into(), password: ANA_PASSWORD.into() })
}

#[tokio::test]
async fn a_member_is_told_when_the_owner_changes_her_access_to_the_organization() {
    let hub = spin_up().await;
    let (owner, owner_sink) = connect(hub.addr, "desktop-owner", owner_key(), None);
    connected(&owner).await;
    let (member, sink) = connect(hub.addr, "desktop-ana", ana(), None);
    connected(&member).await;

    let set = |access: &'static str| {
        let owner = owner.clone();
        async move {
            let reply = owner
                .request(|request_id| Ok(ClientMessage::SetUserOrgAccess { request_id, pairing_key: KEY.into(), id: "ana".into(), access: access.into() }), Duration::from_secs(10))
                .await
                .unwrap();
            assert!(matches!(reply, ServerMessage::UserList { .. }), "{reply:?}");
        }
    };
    let told = |sink: &Arc<Collector>| -> Vec<String> {
        sink.events().into_iter().filter_map(|e| if let RemoteEvent::OrgAccessChanged { access } = e { Some(access) } else { None }).collect()
    };

    set("edit").await;
    until("the new level to reach her", || told(&sink) == ["edit"]).await;
    set("none").await;
    until("the second level to reach her", || told(&sink) == ["edit", "none"]).await;
    assert!(told(&owner_sink).is_empty(), "the owner is not told about a member's access");
    member.stop();
    owner.stop();
}

#[tokio::test]
async fn a_member_signs_in_with_a_password_sees_only_her_conversations_and_returns_by_token() {
    let hub = spin_up().await;
    let (owner, _) = connect(hub.addr, "desktop-owner", owner_key(), None);
    connected(&owner).await;
    owner.turn("owners-only", chat("mine", "owners-only", None), Duration::from_secs(10)).await.unwrap();

    let (member, sink) = connect(hub.addr, "desktop-ana", ana(), None);
    connected(&member).await;
    let RemoteState::Connected { user: Some(user) } = member.state() else { panic!("a member has a user: {:?}", member.state()) };
    assert_eq!(user.id, "ana");
    assert!(!user.must_change_password);
    assert_eq!(sink.tokens().len(), 1);

    let answer = member.turn("hers", chat("hello", "hers", None), Duration::from_secs(10)).await.unwrap();
    assert!(matches!(&answer, ServerMessage::ChatResponse { content, .. } if content == "echo:hello"), "{answer:?}");
    assert_eq!(list(&member).await, vec!["hers"], "not the owner's conversation");
    assert_eq!(list(&owner).await, vec!["owners-only"], "and the owner doesn't see hers");
    let token = sink.tokens().remove(0);
    member.stop();

    // Back with the token alone, no password: the hub still says whose device this is.
    let (back, back_sink) = connect(hub.addr, "desktop-ana", None, Some(token));
    connected(&back).await;
    assert!(matches!(back.state(), RemoteState::Connected { user: Some(ref u) } if u.id == "ana"), "{:?}", back.state());
    assert!(back_sink.tokens().is_empty());
    assert_eq!(list(&back).await, vec!["hers"]);
    back.stop();

    let (wrong, _) = connect(hub.addr, "desktop-ana-2", Some(RemoteCredential::Member { username: "ana".into(), password: "not-her-password".into() }), None);
    until("a wrong password to end it", || matches!(wrong.state(), RemoteState::Stopped { error: Some(_) })).await;
    let RemoteState::Stopped { error: Some(error) } = wrong.state() else { unreachable!() };
    assert!(error.contains("wrong username or password"), "{error}");
    owner.stop();
}

#[tokio::test]
async fn the_link_drops_and_the_client_comes_back_with_the_token_not_the_key() {
    let hub = spin_up().await;
    let proxy = Proxy::to(hub.addr).await;
    let (handle, sink) = connect(proxy.addr, "desktop-1", owner_key(), None);
    connected(&handle).await;
    assert!(list(&handle).await.is_empty());

    proxy.cut();
    until("it to notice", || sink.states().iter().any(|s| matches!(s, RemoteState::Retrying { .. }))).await;
    until("it to come back", || sink.states().iter().filter(|s| matches!(s, RemoteState::Connected { .. })).count() == 2).await;
    assert_eq!(sink.tokens().len(), 1, "the second sign-in used the token: the hub issued nothing new");

    handle.turn("c1", chat("after", "c1", None), Duration::from_secs(10)).await.unwrap();
    assert_eq!(history(&handle, "c1").await.len(), 2);
    handle.stop();
}

#[tokio::test]
async fn what_is_waiting_when_the_link_drops_fails_at_once_and_a_request_while_away_says_so() {
    let hub = spin_up().await;
    let proxy = Proxy::to(hub.addr).await;
    let (handle, sink) = connect(proxy.addr, "desktop-1", owner_key(), None);
    connected(&handle).await;

    let turn = tokio::spawn({
        let handle = handle.clone();
        async move { handle.turn("slow", chat("VERYSLOW", "slow", None), Duration::from_secs(60)).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    proxy.cut();
    let err = turn.await.unwrap().expect_err("the turn can't be answered any more");
    assert!(err.contains("dropped"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(3), "it did not wait for the 8 s model: {:?}", started.elapsed());

    until("it to notice", || matches!(handle.state(), RemoteState::Retrying { .. })).await;
    let away = handle.request(|request_id| Ok(ClientMessage::ListConversations { request_id }), Duration::from_secs(5)).await;
    // Retrying lasts about a second and then it is back; a call in that window is refused, one after it works.
    match away {
        Err(message) => assert!(message.contains("not connected") || message.contains("dropped"), "{message}"),
        Ok(_) => assert!(sink.states().iter().filter(|s| matches!(s, RemoteState::Connected { .. })).count() >= 2),
    }
    handle.stop();
}

#[tokio::test]
async fn turns_of_different_conversations_run_together_and_the_same_conversation_waits_its_turn() {
    let hub = spin_up().await;
    let (handle, _) = connect(hub.addr, "desktop-1", owner_key(), None);
    connected(&handle).await;

    // A first turn warms the hub up (a debug build's cold start can cost hundreds of ms), then one slow turn alone sets
    // what a turn costs here; two in two conversations must take clearly less than two of those one after the other.
    handle.turn("warm", chat("warm up", "warm", None), Duration::from_secs(20)).await.unwrap();
    let started = Instant::now();
    handle.turn("alone", chat("SLOW alone", "alone", None), Duration::from_secs(20)).await.unwrap();
    let alone = started.elapsed();

    let started = Instant::now();
    let (a, b) = tokio::join!(
        handle.turn("one", chat("SLOW a", "one", None), Duration::from_secs(20)),
        handle.turn("two", chat("SLOW b", "two", None), Duration::from_secs(20)),
    );
    let together = started.elapsed();
    assert!(a.is_ok() && b.is_ok());
    assert!(together < alone * 3 / 2, "two conversations should overlap: {together:?} for two, {alone:?} for one");

    let started = Instant::now();
    let (first, second) = tokio::join!(
        handle.turn("same", chat("SLOW first", "same", None), Duration::from_secs(20)),
        handle.turn("same", chat("SLOW second", "same", None), Duration::from_secs(20)),
    );
    assert!(started.elapsed() >= SLOW * 2, "the second turn of a conversation starts after the first answered, took {:?}", started.elapsed());
    assert!(matches!(first.unwrap(), ServerMessage::ChatResponse { content, .. } if content == "echo:SLOW first"));
    assert!(matches!(second.unwrap(), ServerMessage::ChatResponse { content, .. } if content == "echo:SLOW second"));
    let turns: Vec<_> = history(&handle, "same").await.into_iter().map(|(_, text)| text).collect();
    assert_eq!(turns, vec!["SLOW first", "echo:SLOW first", "SLOW second", "echo:SLOW second"], "in order, nothing lost");
    handle.stop();
}

#[tokio::test]
async fn an_approval_goes_to_the_person_and_the_answer_comes_back() {
    for approve in [false, true] {
        let hub = spin_up().await;
        let (handle, sink) = connect(hub.addr, "desktop-1", owner_key(), None);
        connected(&handle).await;

        let turn = tokio::spawn({
            let handle = handle.clone();
            async move { handle.turn("c1", chat("CREATE a critic", "c1", Some("chief")), Duration::from_secs(20)).await }
        });
        until("the approval to reach the sink", || sink.events().iter().any(|e| matches!(e, RemoteEvent::Approval { .. }))).await;
        let Some(RemoteEvent::Approval { approval_id, action, target, .. }) = sink.events().into_iter().find(|e| matches!(e, RemoteEvent::Approval { .. })) else { unreachable!() };
        assert_eq!((action.as_str(), target.as_str()), ("create_agent", "critic"));

        handle.send(ClientMessage::ResolveApproval { approval_id, approved: approve, always: false });
        let reply = turn.await.unwrap().unwrap();
        assert!(matches!(reply, ServerMessage::ChatResponse { .. }), "approve={approve}: {reply:?}");
        let agents = load_config_from_path(&hub.config_path, false).unwrap().agents;
        assert_eq!(agents.iter().any(|a| a.id == "critic"), approve, "only a yes saves the agent");
        handle.stop();
    }
}

#[tokio::test]
async fn an_approval_still_open_when_the_link_drops_is_closed_for_the_person() {
    let hub = spin_up().await;
    let proxy = Proxy::to(hub.addr).await;
    let (handle, sink) = connect(proxy.addr, "desktop-1", owner_key(), None);
    connected(&handle).await;

    let turn = tokio::spawn({
        let handle = handle.clone();
        async move { handle.turn("c1", chat("CREATE a critic", "c1", Some("chief")), Duration::from_secs(20)).await }
    });
    until("the approval", || sink.events().iter().any(|e| matches!(e, RemoteEvent::Approval { .. }))).await;
    let Some(RemoteEvent::Approval { approval_id, .. }) = sink.events().into_iter().find(|e| matches!(e, RemoteEvent::Approval { .. })) else { unreachable!() };
    proxy.cut();
    assert!(turn.await.unwrap().is_err());
    until("the modal to be told to close", || sink.events().iter().any(|e| matches!(e, RemoteEvent::ApprovalCancelled { approval_id: id } if *id == approval_id))).await;
    handle.stop();
}

#[tokio::test]
async fn a_device_the_owner_revokes_is_cut_off_for_good_not_retried() {
    let hub = spin_up().await;
    let (handle, sink) = connect(hub.addr, "desktop-1", owner_key(), None);
    connected(&handle).await;

    PairingStore::new(&hub.devices_path).revoke("desktop-1").unwrap();
    until("it to be turned away", || matches!(handle.state(), RemoteState::Stopped { error: Some(_) })).await;
    let RemoteState::Stopped { error: Some(error) } = handle.state() else { unreachable!() };
    assert!(error.contains("device revoked"), "{error}");
    // Ended: it didn't go through Retrying to try again.
    assert!(!sink.states().iter().any(|s| matches!(s, RemoteState::Retrying { .. })), "{:?}", sink.states());
    let refused = handle.request(|request_id| Ok(ClientMessage::ListConversations { request_id }), Duration::from_secs(2)).await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn a_hub_that_is_not_there_is_retried_and_asking_meanwhile_says_so() {
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let nowhere = free.local_addr().unwrap();
    drop(free);
    let (handle, sink) = connect(nowhere, "desktop-1", owner_key(), None);
    until("a retry", || sink.states().iter().any(|s| matches!(s, RemoteState::Retrying { .. }))).await;
    let err = handle.request(|request_id| Ok(ClientMessage::ListConversations { request_id }), Duration::from_secs(5)).await.expect_err("nothing is there");
    assert!(err.contains("not connected"), "{err}");
    handle.stop();
    until("stopped by the person", || sink.states().last() == Some(&RemoteState::Stopped { error: None })).await;
}
