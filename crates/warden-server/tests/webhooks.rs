//! P105 on the hub: `POST /hooks/<id>` with a webhook's token runs its agent with the request as data, answers `202` at
//! once, and the result lands in the webhook's conversation, which every device hears about and lists. Spoken to with
//! plain HTTP over a real socket, next to the web page and the WebSocket protocol on the same port.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use warden_bootstrap::tasks::TaskStore;
use warden_bootstrap::webhooks::{WebhookAuth, WebhookConfig};
use warden_bootstrap::{load_conversation, save_config, AgentConfig, ChatRole, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{ChatStream, Message, ModelProvider, Role, StreamEvent, Usage};
use warden_core::orchestrator::Orchestrator;
use warden_core::spend::{Limit, MemoryStore, PriceTable, Scope, SpendGuard};
use warden_core::tool::ToolSpec;
use warden_server::webhook_tokens::WebhookTokenStore;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost, StaticWebUi};

const KEY: &str = "pairing-key-0123456789-0123456789";

/// The poet answers "haiku!" with 10 tokens of usage; anyone else "plain". Keeps every input it was given, and a
/// call whose input has `HOLD` waits for a permit on `gate` before answering.
struct Scripted {
    inputs: Arc<Mutex<Vec<String>>>,
    offered: Arc<Mutex<Vec<Vec<String>>>>,
    gate: Arc<tokio::sync::Semaphore>,
}

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let system = messages.iter().filter(|m| m.role == Role::System).map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
        let last = messages.last().unwrap().content.clone();
        self.inputs.lock().unwrap().push(last.clone());
        self.offered.lock().unwrap().push(tools.iter().map(|t| t.name.clone()).collect());
        if last.contains("HOLD") {
            self.gate.acquire().await.unwrap().forget();
        }
        let text = if system.contains("You are the poet") { "haiku!" } else { "plain" };
        let events = vec![Ok(StreamEvent::ContentDelta(text.into())), Ok(StreamEvent::Usage(Usage { prompt_tokens: 7, completion_tokens: 3, total_tokens: 10 }))];
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}

struct Host {
    path: PathBuf,
}

#[async_trait]
impl SettingsHost for Host {
    fn config_path(&self) -> PathBuf {
        self.path.clone()
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        anyhow::bail!("not used by these tests")
    }
}

fn agent(id: &str, persona: &str) -> AgentConfig {
    AgentConfig {
        id: id.into(),
        persona: persona.into(),
        provider_id: None,
        can_delegate_to_agents: false,
        can_manage_agents: false,
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
    }
}

fn hook(id: &str, agent: Option<&str>, prompt: &str) -> WebhookConfig {
    WebhookConfig { id: id.into(), agent: agent.map(str::to_string), prompt: prompt.into(), enabled: true, auth: WebhookAuth::Token }
}

struct Hub {
    addr: SocketAddr,
    config_path: PathBuf,
    store: TaskStore,
    tokens: WebhookTokenStore,
    inputs: Arc<Mutex<Vec<String>>>,
    offered: Arc<Mutex<Vec<Vec<String>>>>,
    gate: Arc<tokio::sync::Semaphore>,
}

impl Hub {
    fn set_webhooks(&self, webhooks: Vec<WebhookConfig>) {
        let config = FileConfig { agents: vec![agent("poet", "You are the poet.")], webhooks, ..FileConfig::default() };
        save_config(&self.config_path, &config).unwrap();
    }

    fn conversation(&self, id: &str) -> Option<warden_bootstrap::Conversation> {
        load_conversation(&self.store.conversations_dir(), id).unwrap()
    }

    /// Waits until the conversation has `messages` messages.
    async fn conversation_with(&self, id: &str, messages: usize) -> warden_bootstrap::Conversation {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(conversation) = self.conversation(id).filter(|c| c.messages.len() >= messages) {
                return conversation;
            }
            assert!(Instant::now() < deadline, "the conversation '{id}' never got {messages} messages");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn post(&self, id: &str, token: Option<&str>, body: &str) -> (u16, String) {
        http(self.addr, "POST", &format!("/hooks/{id}"), token.map(|t| ("Authorization", format!("Bearer {t}"))), Some(body)).await
    }
}

async fn hub_with(limits: Vec<Limit>, webhooks_on: bool) -> Hub {
    let dir = std::env::temp_dir().join(format!("warden-server-webhooks-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let offered = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let guard = Arc::new(SpendGuard::new(Arc::new(MemoryStore::default()), limits, PriceTable::new(Vec::new())));
    let model = Scripted { inputs: inputs.clone(), offered: offered.clone(), gate: gate.clone() };
    let orchestrator = Orchestrator::new(Arc::new(model), Arc::new(Vault::new(dir.join("vault")))).with_spend_guard(guard);
    let store = TaskStore::new(dir.join("tasks"));
    let web = StaticWebUi([("index.html".to_string(), b"<p>web</p>".to_vec())].into_iter().collect());
    let mut server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(Host { path: config_path.clone() }))
        .with_web_ui(Arc::new(web))
        .with_tasks(store.clone(), false);
    if webhooks_on {
        server = server.with_webhooks(dir.join("webhook_tokens.json"));
    }
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    let hub = Hub { addr, config_path, store, tokens: WebhookTokenStore::new(dir.join("webhook_tokens.json")), inputs, offered, gate };
    hub.set_webhooks(vec![hook("build", Some("poet"), "Why did the build fail?")]);
    hub
}

async fn hub() -> Hub {
    hub_with(Vec::new(), true).await
}

/// One request, the whole answer: (status code, body). `body: None` sends no `Content-Length` at all.
async fn http(addr: SocketAddr, method: &str, path: &str, header: Option<(&str, String)>, body: Option<&str>) -> (u16, String) {
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n");
    if let Some((name, value)) = header {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body.unwrap_or_default());
    raw_request(addr, request.as_bytes()).await
}

async fn raw_request(addr: SocketAddr, request: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut raw = String::new();
    // The server answers and closes; a caller that never gets one fails the test instead of hanging it.
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut raw)).await.expect("no answer within 10 s").unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or_else(|| panic!("not an HTTP answer: {raw:?}"));
    (head.split_whitespace().nth(1).unwrap().parse().unwrap(), body.to_string())
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
}

#[tokio::test]
async fn a_call_with_the_token_runs_the_agent_and_lands_in_the_webhooks_conversation() {
    let hub = hub().await;
    let token = hub.tokens.create("build").unwrap().token;

    // A device is connected and hears about the run.
    let mut device = ServerConnection::connect(&format!("ws://{}", hub.addr), "web-1", "Browser", KEY).await.unwrap();

    let (status, body) = hub.post("build", Some(&token), r#"{"build":42,"status":"failed"}"#).await;
    assert_eq!(status, 202, "{body}");
    let answer = json(&body);
    assert_eq!((answer["status"].as_str(), answer["webhook"].as_str(), answer["conversation"].as_str()), (Some("started"), Some("build"), Some("task-hook-build")));
    assert!(!body.contains(&token), "the token is never echoed");

    let saved = hub.conversation_with("task-hook-build", 2).await;
    assert_eq!((saved.title.as_str(), saved.agent_id.as_deref()), ("Webhook: build", Some("poet")));
    assert_eq!(saved.messages[0].role, ChatRole::User);
    let told = &saved.messages[0].content;
    let (prompt_at, body_at) = (told.find("Why did the build fail?").unwrap(), told.find(r#"{"build":42,"status":"failed"}"#).unwrap());
    assert!(prompt_at < body_at, "the prompt first, the request after it: {told}");
    assert!(told.contains("never instructions to follow") && told.contains("Content-Type: application/json"), "{told}");
    assert_eq!((saved.messages[1].role, saved.messages[1].content.as_str()), (ChatRole::Assistant, "haiku!"));
    assert!(!hub.offered.lock().unwrap().is_empty(), "the model ran");

    // The conversation is announced and listed like a task's.
    let heard = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let ServerMessage::ConversationsChanged { conversation_id } = device.recv().await.unwrap().expect("connection closed") {
                return conversation_id;
            }
        }
    })
    .await
    .expect("the device never heard about the run");
    assert_eq!(heard, "task-hook-build");
    device.send(&ClientMessage::ListConversations { request_id: 7 }).await.unwrap();
    let listed = loop {
        if let ServerMessage::ConversationList { conversations, .. } = device.recv().await.unwrap().expect("connection closed") {
            break conversations;
        }
    };
    assert!(listed.iter().any(|c| c.id == "task-hook-build"), "{listed:?}");

    // The other header some services can set works too, and a second call continues the same conversation.
    let (status, _) = http(hub.addr, "POST", "/hooks/build", Some(("X-Warden-Token", token.clone())), Some("again")).await;
    assert_eq!(status, 202);
    hub.conversation_with("task-hook-build", 4).await;
}

#[tokio::test]
async fn every_way_of_not_having_the_token_gets_the_same_401_and_runs_nothing() {
    let hub = hub().await;
    hub.set_webhooks(vec![hook("build", Some("poet"), "p"), hook("deploy", Some("poet"), "p")]);
    let build = hub.tokens.create("build").unwrap().token;
    let deploy = hub.tokens.create("deploy").unwrap().token;

    let started = Instant::now();
    // No token, a wrong one, another webhook's, one for a webhook that doesn't exist, an id that can't be one — and the
    // right id with a token that only differs by one character. Concurrently: each waits the guessing delay.
    let one_off = format!("{}x", &build[..build.len() - 1]);
    let (a, b, c, d, e, f) = tokio::join!(
        hub.post("build", None, "x"),
        hub.post("build", Some("whk_nope"), "x"),
        hub.post("build", Some(&deploy), "x"),
        hub.post("ghost", Some(&build), "x"),
        hub.post("not.an.id", Some(&build), "x"),
        hub.post("build", Some(&one_off), "x"),
    );
    for (status, body) in [&a, &b, &c, &d, &e, &f] {
        assert_eq!(*status, 401, "{body}");
    }
    assert!(started.elapsed() >= Duration::from_millis(900), "a wrong token waits before it is told so");
    assert!(a.1 == b.1 && b.1 == c.1 && c.1 == d.1 && d.1 == e.1 && e.1 == f.1, "the answer doesn't say which part was wrong: {a:?} {d:?}");
    assert_eq!(json(&a.1)["error"]["code"], "invalid_token");

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(hub.inputs.lock().unwrap().is_empty(), "the model was never called");
    assert!(hub.conversation("task-hook-build").is_none() && hub.conversation("task-hook-deploy").is_none(), "nothing was saved");
}

#[tokio::test]
async fn the_token_is_checked_before_the_body_is_read() {
    let hub = hub().await;
    // A body of 200 KiB announced and never sent: if the hub waited for it, the answer would come after its 30 s timeout.
    let head = "POST /hooks/build HTTP/1.1\r\nHost: x\r\nContent-Length: 200000\r\n\r\n";
    let started = Instant::now();
    let (status, _) = raw_request(hub.addr, head.as_bytes()).await;
    assert_eq!(status, 401);
    assert!(started.elapsed() < Duration::from_secs(5), "the hub waited for a body from a caller with no token");
}

#[tokio::test]
async fn a_paused_or_removed_webhook_is_refused_and_a_resumed_one_works_without_a_restart() {
    let hub = hub().await;
    let token = hub.tokens.create("build").unwrap().token;

    hub.set_webhooks(vec![WebhookConfig { enabled: false, ..hook("build", Some("poet"), "p") }]);
    let (status, body) = hub.post("build", Some(&token), "x").await;
    assert_eq!((status, json(&body)["error"]["code"].as_str()), (403, Some("webhook_paused")));

    hub.set_webhooks(Vec::new());
    let (status, body) = hub.post("build", Some(&token), "x").await;
    assert_eq!((status, json(&body)["error"]["code"].as_str()), (404, Some("unknown_webhook")), "the token still opens nothing once the webhook is gone");

    hub.set_webhooks(vec![hook("build", Some("poet"), "p")]);
    assert_eq!(hub.post("build", Some(&token), "x").await.0, 202);
    hub.conversation_with("task-hook-build", 2).await;
}

#[tokio::test]
async fn what_is_not_a_clean_post_is_refused_before_the_model() {
    let hub = hub().await;
    let token = hub.tokens.create("build").unwrap().token;
    let bearer = Some(("Authorization", format!("Bearer {token}")));

    assert_eq!(http(hub.addr, "GET", "/hooks/build", bearer.clone(), None).await.0, 405);
    assert_eq!(http(hub.addr, "PUT", "/hooks/build", bearer.clone(), Some("x")).await.0, 405);
    // No Content-Length, and a chunked body, are both asked to send one.
    assert_eq!(http(hub.addr, "POST", "/hooks/build", bearer.clone(), None).await.0, 411);
    let chunked = format!("POST /hooks/build HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n0\r\n\r\n");
    assert_eq!(raw_request(hub.addr, chunked.as_bytes()).await.0, 411);
    let not_a_number = format!("POST /hooks/build HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: lots\r\n\r\n");
    assert_eq!(raw_request(hub.addr, not_a_number.as_bytes()).await.0, 400);
    // Over the limit is refused from the header alone, with nothing sent.
    let huge = format!("POST /hooks/build HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n", warden_server::webhooks::MAX_BODY_BYTES + 1);
    assert_eq!(raw_request(hub.addr, huge.as_bytes()).await.0, 413);
    // A body that stops short of its Content-Length is a bad request.
    let short = format!("POST /hooks/build HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: 50\r\n\r\nonly a little");
    let mut stream = TcpStream::connect(hub.addr).await.unwrap();
    stream.write_all(short.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    assert!(raw.starts_with("HTTP/1.1 400"), "{raw}");

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(hub.inputs.lock().unwrap().is_empty(), "the model was never called");

    // The biggest body that fits is accepted — and is cut for the model.
    let big = "é".repeat(warden_server::webhooks::MAX_BODY_BYTES / 2);
    assert_eq!(hub.post("build", Some(&token), &big).await.0, 202);
    let saved = hub.conversation_with("task-hook-build", 2).await;
    assert!(saved.messages[0].content.contains("cut to the first"), "the model got only the start of it");
}

#[tokio::test]
async fn a_call_while_the_last_one_still_works_is_a_409_and_then_it_works_again() {
    let hub = hub().await;
    let token = hub.tokens.create("build").unwrap().token;

    assert_eq!(hub.post("build", Some(&token), "HOLD").await.0, 202);
    // The first call is working (held by the model), so the next is not queued behind it.
    let (status, body) = hub.post("build", Some(&token), "second").await;
    assert_eq!((status, json(&body)["error"]["code"].as_str()), (409, Some("still_running")));
    assert!(hub.inputs.lock().unwrap().iter().all(|i| !i.contains("second")), "the refused call never reached the model");

    hub.gate.add_permits(1);
    hub.conversation_with("task-hook-build", 2).await;
    // Once it finishes the webhook takes calls again (the running mark is dropped just after the save).
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if hub.post("build", Some(&token), "third").await.0 == 202 {
            break;
        }
        assert!(Instant::now() < deadline, "the webhook never took a call again");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    hub.conversation_with("task-hook-build", 4).await;
}

#[tokio::test]
async fn a_new_token_replaces_the_old_one_and_a_revoked_one_stops_working_on_the_next_call() {
    let hub = hub().await;
    let old = hub.tokens.create("build").unwrap().token;
    assert_eq!(hub.post("build", Some(&old), "x").await.0, 202);
    hub.conversation_with("task-hook-build", 2).await;

    let new = hub.tokens.create("build").unwrap().token;
    assert_eq!(hub.post("build", Some(&old), "x").await.0, 401, "the rotated token is dead at once");
    let deadline = Instant::now() + Duration::from_secs(10);
    while hub.post("build", Some(&new), "x").await.0 != 202 {
        assert!(Instant::now() < deadline, "the new token never worked");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(hub.tokens.revoke("build").unwrap());
    assert_eq!(hub.post("build", Some(&new), "x").await.0, 401);
}

#[tokio::test]
async fn the_spending_limits_apply_to_the_webhooks_channel() {
    // 5 tokens a day on the channel `webhooks`; the model uses 10 per answer.
    let limit = Limit::new("hooks", Scope::Channel("webhooks".into()), 24).with_max_tokens(5);
    let hub = hub_with(vec![limit], true).await;
    let token = hub.tokens.create("build").unwrap().token;

    assert_eq!(hub.post("build", Some(&token), "first").await.0, 202);
    let first = hub.conversation_with("task-hook-build", 2).await;
    assert_eq!(first.messages[1].content, "haiku!", "under the limit it runs");

    let deadline = Instant::now() + Duration::from_secs(10);
    while hub.post("build", Some(&token), "second").await.0 != 202 {
        assert!(Instant::now() < deadline, "the webhook never took a second call");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let second = hub.conversation_with("task-hook-build", 4).await;
    let note = &second.messages[3].content;
    assert!(note.to_lowercase().contains("limit") && note != "haiku!", "over the limit the call is stopped and says so: {note}");
    assert_eq!(hub.inputs.lock().unwrap().len(), 1, "the model was not called the second time");
}

#[tokio::test]
async fn a_hub_without_webhooks_answers_404_and_never_the_web_page() {
    let hub = hub_with(Vec::new(), false).await;
    let (status, body) = hub.post("build", Some("whk_anything"), "x").await;
    assert_eq!(status, 404, "{body}");
    assert!(!body.contains("<p>web</p>"), "the web page's fallback must not answer a webhook call");
    assert_eq!(http(hub.addr, "GET", "/", None, None).await.0, 200, "the page itself is still served");
}

// ---- administration from a client (the screens) ----

async fn owner(hub: &Hub) -> ServerConnection {
    ServerConnection::connect(&format!("ws://{}", hub.addr), "web-1", "Browser", KEY).await.unwrap()
}

/// Sends `message` and returns the first webhook reply (the hub may also send other things in between).
async fn ask(conn: &mut ServerConnection, message: ClientMessage) -> ServerMessage {
    conn.send(&message).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match conn.recv().await.unwrap().expect("connection closed") {
                reply @ (ServerMessage::WebhookList { .. } | ServerMessage::WebhookCreated { .. } | ServerMessage::WebhookError { .. }) => return reply,
                _ => continue,
            }
        }
    })
    .await
    .expect("no webhook reply within 10 s")
}

fn dto(id: &str, auth: &str) -> warden_server_protocol::protocol::WebhookDto {
    warden_server_protocol::protocol::WebhookDto { id: id.into(), agent_id: Some("poet".into()), prompt: "Why did the build fail?".into(), enabled: true, auth: auth.into() }
}

fn save(original: Option<&str>, webhook: warden_server_protocol::protocol::WebhookDto) -> ClientMessage {
    ClientMessage::SaveWebhook { request_id: 1, pairing_key: KEY.into(), original_id: original.map(str::to_string), webhook }
}

fn list_of(reply: ServerMessage) -> Vec<warden_server_protocol::protocol::WebhookInfoDto> {
    match reply {
        ServerMessage::WebhookList { webhooks, .. } => webhooks,
        other => panic!("expected WebhookList, got {other:?}"),
    }
}

fn change(message: fn(u64, String, String) -> ClientMessage, id: &str) -> ClientMessage {
    message(1, KEY.to_string(), id.to_string())
}

#[tokio::test]
async fn a_screen_lists_makes_and_uses_a_webhook_end_to_end() {
    let hub = hub().await;
    hub.set_webhooks(Vec::new());
    let mut me = owner(&hub).await;

    // Empty at first, and the hub says it takes calls.
    match ask(&mut me, ClientMessage::ListWebhooks { request_id: 1 }).await {
        ServerMessage::WebhookList { webhooks, serves_here, .. } => assert_eq!((webhooks.len(), serves_here), (0, true)),
        other => panic!("{other:?}"),
    }

    // A webhook made on the screen: no credential yet, so it takes no calls.
    let listed = list_of(ask(&mut me, save(None, dto("build", "token"))).await);
    assert_eq!((listed[0].id.as_str(), listed[0].auth.as_str(), listed[0].credential.as_deref(), listed[0].conversation.as_str()), ("build", "token", None, "task-hook-build"));
    assert_eq!(hub.post("build", Some("whk_nothing"), "x").await.0, 401);

    // The credential comes once, with the list that shows it.
    let (id, credential, kind, listed) = match ask(&mut me, change(|r, k, id| ClientMessage::CreateWebhookCredential { request_id: r, pairing_key: k, id }, "build")).await {
        ServerMessage::WebhookCreated { id, credential, kind, webhooks, .. } => (id, credential, kind, webhooks),
        other => panic!("{other:?}"),
    };
    assert_eq!((id.as_str(), kind.as_str()), ("build", "token"));
    assert!(credential.starts_with("whk_"));
    assert_eq!((listed[0].credential.as_deref(), listed[0].shown.as_deref()), (Some("token"), Some(&credential[..10])));
    let again = list_of(ask(&mut me, ClientMessage::ListWebhooks { request_id: 2 }).await);
    assert!(!format!("{again:?}").contains(&credential), "a later list never carries the credential");

    // It works as a call, and making another one replaces it at once.
    assert_eq!(hub.post("build", Some(&credential), "x").await.0, 202);
    hub.conversation_with("task-hook-build", 2).await;
    let ServerMessage::WebhookCreated { credential: second, .. } = ask(&mut me, change(|r, k, id| ClientMessage::CreateWebhookCredential { request_id: r, pairing_key: k, id }, "build")).await else { panic!() };
    assert_ne!(second, credential);
    assert_eq!(hub.post("build", Some(&credential), "x").await.0, 401, "the rotated credential is dead at once");
}

#[tokio::test]
async fn a_signing_secret_is_made_for_an_hmac_webhook_and_a_change_of_mode_takes_it_away() {
    let hub = hub().await;
    hub.set_webhooks(Vec::new());
    let mut me = owner(&hub).await;
    list_of(ask(&mut me, save(None, dto("gh", "hmac"))).await);

    let ServerMessage::WebhookCreated { credential: secret, kind, webhooks, .. } = ask(&mut me, change(|r, k, id| ClientMessage::CreateWebhookCredential { request_id: r, pairing_key: k, id }, "gh")).await else { panic!() };
    assert!(secret.starts_with("whsec_") && kind == "hmac" && webhooks[0].credential.as_deref() == Some("hmac"));
    let body = r#"{"ref":"main"}"#;
    assert_eq!(post_with(&hub, "gh", &[github_signature(&secret, body)], body).await.0, 202);
    hub.conversation_with("task-hook-gh", 2).await;

    // Switched to tokens on the screen: the secret was the wrong kind, so it is gone, and the list says so.
    let listed = list_of(ask(&mut me, save(Some("gh"), dto("gh", "token"))).await);
    assert_eq!((listed[0].auth.as_str(), listed[0].credential.as_deref()), ("token", None));
    assert_eq!(hub.tokens.kind_of("gh").unwrap(), None);
    assert_eq!(post_with(&hub, "gh", &[github_signature(&secret, body)], body).await.0, 401, "a signature no longer opens it");
    let ServerMessage::WebhookCreated { credential: token, kind, .. } = ask(&mut me, change(|r, k, id| ClientMessage::CreateWebhookCredential { request_id: r, pairing_key: k, id }, "gh")).await else { panic!() };
    assert!(token.starts_with("whk_") && kind == "token", "a new credential is of the kind the webhook now wants");

    // Saving without changing the mode keeps the credential.
    list_of(ask(&mut me, save(Some("gh"), warden_server_protocol::protocol::WebhookDto { prompt: "a new prompt".into(), ..dto("gh", "token") })).await);
    let deadline = Instant::now() + Duration::from_secs(10);
    while hub.post("gh", Some(&token), "x").await.0 != 202 {
        assert!(Instant::now() < deadline, "the token stopped working after an edit that didn't touch the mode");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_rename_keeps_the_credential_and_pause_delete_and_revoke_take_effect_on_the_next_call() {
    let hub = hub().await;
    hub.set_webhooks(Vec::new());
    let mut me = owner(&hub).await;
    list_of(ask(&mut me, save(None, dto("build", "token"))).await);
    let ServerMessage::WebhookCreated { credential, .. } = ask(&mut me, change(|r, k, id| ClientMessage::CreateWebhookCredential { request_id: r, pairing_key: k, id }, "build")).await else { panic!() };

    // Renamed: the credential moves with the name.
    let listed = list_of(ask(&mut me, save(Some("build"), dto("ci", "token"))).await);
    assert_eq!((listed[0].id.as_str(), listed[0].credential.as_deref()), ("ci", Some("token")));
    assert_eq!(hub.post("build", Some(&credential), "x").await.0, 401, "the old name is gone");
    assert_eq!(hub.post("ci", Some(&credential), "x").await.0, 202);
    hub.conversation_with("task-hook-ci", 2).await;
    wait_until_ci_is_free(&hub, &credential).await;

    // Paused: refused, with the right token. Resumed: works.
    let listed = list_of(ask(&mut me, ClientMessage::SetWebhookEnabled { request_id: 1, pairing_key: KEY.into(), id: "ci".into(), enabled: false }).await);
    assert!(!listed[0].enabled);
    assert_eq!(hub.post("ci", Some(&credential), "x").await.0, 403);
    list_of(ask(&mut me, ClientMessage::SetWebhookEnabled { request_id: 1, pairing_key: KEY.into(), id: "ci".into(), enabled: true }).await);

    // Revoked: the webhook stays, without a credential; a second revoke says there is none.
    let listed = list_of(ask(&mut me, change(|r, k, id| ClientMessage::RevokeWebhookCredential { request_id: r, pairing_key: k, id }, "ci")).await);
    assert_eq!((listed.len(), listed[0].credential.as_deref()), (1, None));
    assert_eq!(hub.post("ci", Some(&credential), "x").await.0, 401);
    assert!(matches!(ask(&mut me, change(|r, k, id| ClientMessage::RevokeWebhookCredential { request_id: r, pairing_key: k, id }, "ci")).await, ServerMessage::WebhookError { auth_rejected: false, .. }));

    // Deleted: the config entry and the credential are both gone.
    let ServerMessage::WebhookCreated { .. } = ask(&mut me, change(|r, k, id| ClientMessage::CreateWebhookCredential { request_id: r, pairing_key: k, id }, "ci")).await else { panic!() };
    assert!(hub.tokens.kind_of("ci").unwrap().is_some());
    let listed = list_of(ask(&mut me, change(|r, k, id| ClientMessage::DeleteWebhook { request_id: r, pairing_key: k, id }, "ci")).await);
    assert!(listed.is_empty());
    assert_eq!(hub.tokens.kind_of("ci").unwrap(), None, "a removed webhook leaves no credential behind");
}

/// Waits until `ci` takes a call again (the running mark is dropped just after the conversation is saved).
async fn wait_until_ci_is_free(hub: &Hub, token: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if hub.post("ci", Some(token), "probe").await.0 == 202 {
            hub.conversation_with("task-hook-ci", 4).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            return;
        }
        assert!(Instant::now() < deadline, "the webhook never took a call again");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_pairing_key_guards_every_change_and_a_refused_one_changes_nothing() {
    let hub = hub().await;
    let mut me = owner(&hub).await;
    let wrong = "not-the-pairing-key-0123456789-0123456789".to_string();
    let attempts = [
        ClientMessage::SaveWebhook { request_id: 1, pairing_key: wrong.clone(), original_id: None, webhook: dto("evil", "token") },
        ClientMessage::SetWebhookEnabled { request_id: 2, pairing_key: wrong.clone(), id: "build".into(), enabled: false },
        ClientMessage::DeleteWebhook { request_id: 3, pairing_key: wrong.clone(), id: "build".into() },
        ClientMessage::CreateWebhookCredential { request_id: 4, pairing_key: wrong.clone(), id: "build".into() },
        ClientMessage::RevokeWebhookCredential { request_id: 5, pairing_key: wrong.clone(), id: "build".into() },
    ];
    for attempt in attempts {
        let started = Instant::now();
        match ask(&mut me, attempt).await {
            ServerMessage::WebhookError { auth_rejected: true, .. } => {}
            other => panic!("expected a rejected key, got {other:?}"),
        }
        assert!(started.elapsed() >= Duration::from_millis(900), "a wrong key waits before it is told so");
    }
    // Nothing changed: the webhook is still there and enabled, and no credential was made.
    let listed = list_of(ask(&mut me, ClientMessage::ListWebhooks { request_id: 9 }).await);
    assert_eq!((listed.len(), listed[0].id.as_str(), listed[0].enabled, listed[0].credential.clone()), (1, "build", true, None));
    assert_eq!(hub.tokens.kind_of("build").unwrap(), None);
}

#[tokio::test]
async fn a_bad_form_is_refused_with_a_reason_and_a_member_never_gets_to_the_webhooks() {
    let hub = hub().await;
    let mut me = owner(&hub).await;
    for (bad, why) in [
        (dto("bad id", "token"), "invalid webhook id"),
        (dto("ok", "basic"), "unknown webhook auth"),
        (warden_server_protocol::protocol::WebhookDto { agent_id: Some("ghost".into()), ..dto("ok", "token") }, "doesn't exist"),
        (warden_server_protocol::protocol::WebhookDto { prompt: "  ".into(), ..dto("ok", "token") }, "empty prompt"),
        (dto("build", "token"), "already a webhook"),
    ] {
        match ask(&mut me, save(None, bad)).await {
            ServerMessage::WebhookError { message, auth_rejected: false, .. } => assert!(message.contains(why), "{message}"),
            other => panic!("expected a refusal about '{why}', got {other:?}"),
        }
    }
    assert_eq!(list_of(ask(&mut me, ClientMessage::ListWebhooks { request_id: 1 }).await).len(), 1, "none of them was saved");
    match ask(&mut me, change(|r, k, id| ClientMessage::CreateWebhookCredential { request_id: r, pairing_key: k, id }, "ghost")).await {
        ServerMessage::WebhookError { message, .. } => assert!(message.contains("no webhook named 'ghost'"), "{message}"),
        other => panic!("{other:?}"),
    }

    // A member is turned away from every webhook request, before anything is read.
    for message in [
        ClientMessage::ListWebhooks { request_id: 1 },
        ClientMessage::SaveWebhook { request_id: 2, pairing_key: String::new(), original_id: None, webhook: dto("m", "token") },
        ClientMessage::SetWebhookEnabled { request_id: 3, pairing_key: String::new(), id: "build".into(), enabled: false },
        ClientMessage::DeleteWebhook { request_id: 4, pairing_key: String::new(), id: "build".into() },
        ClientMessage::CreateWebhookCredential { request_id: 5, pairing_key: String::new(), id: "build".into() },
        ClientMessage::RevokeWebhookCredential { request_id: 6, pairing_key: String::new(), id: "build".into() },
    ] {
        assert!(matches!(warden_server::people::member_refusal(&message), Some(ServerMessage::WebhookError { auth_rejected: true, .. })), "{message:?}");
    }
}

#[tokio::test]
async fn a_hub_without_webhooks_tells_the_screen_so() {
    let hub = hub_with(Vec::new(), false).await;
    let mut me = owner(&hub).await;
    match ask(&mut me, ClientMessage::ListWebhooks { request_id: 1 }).await {
        ServerMessage::WebhookError { message, .. } => assert!(message.contains("doesn't offer webhooks"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(ask(&mut me, save(None, dto("x", "token"))).await, ServerMessage::WebhookError { .. }));
}

// ---- signed calls (HMAC) ----

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// HMAC-SHA256 of `payload` with `secret`, in hex.
fn sign(secret: &str, payload: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload);
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn github_signature(secret: &str, body: &str) -> (&'static str, String) {
    ("X-Hub-Signature-256", format!("sha256={}", sign(secret, body.as_bytes())))
}

fn stripe_signature(secret: &str, body: &str, at: i64) -> (&'static str, String) {
    ("Stripe-Signature", format!("t={at},v1={}", sign(secret, format!("{at}.{body}").as_bytes())))
}

/// A `POST /hooks/<id>` with any number of headers. The request is built here and now, so the future it returns borrows
/// nothing and several can be awaited together.
fn post_with(hub: &Hub, id: &str, headers: &[(&str, String)], body: &str) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u16, String)>>> {
    let mut request = format!("POST /hooks/{id} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", hub.addr, body.len());
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    let addr = hub.addr;
    Box::pin(async move { raw_request(addr, request.as_bytes()).await })
}

fn signed_hook(id: &str) -> WebhookConfig {
    WebhookConfig { auth: WebhookAuth::Hmac, ..hook(id, Some("poet"), "Why did the build fail?") }
}

#[tokio::test]
async fn a_github_signed_call_runs_the_agent_and_a_bearer_token_does_not_open_it() {
    let hub = hub().await;
    hub.set_webhooks(vec![signed_hook("gh")]);
    let secret = hub.tokens.create_secret("gh").unwrap().token;
    let body = r#"{"ref":"refs/heads/main","after":"abc123"}"#;

    let (status, answer) = post_with(&hub, "gh", &[github_signature(&secret, body), ("X-GitHub-Delivery", "delivery-1".into())], body).await;
    assert_eq!(status, 202, "{answer}");
    assert_eq!(json(&answer)["status"], "started");
    let saved = hub.conversation_with("task-hook-gh", 2).await;
    assert!(saved.messages[0].content.contains("abc123"), "the body reached the model as data: {}", saved.messages[0].content);
    assert_eq!(saved.messages[1].content, "haiku!");
    assert!(hub.tokens.list().unwrap()[0].last_used_at_ms.is_some(), "the use is noted");

    // The secret is not a bearer token: a service that sends it that way is not let in.
    assert_eq!(hub.post("gh", Some(&secret), body).await.0, 401);
}

#[tokio::test]
async fn a_stripe_signed_call_runs_and_a_stale_or_moved_timestamp_is_refused() {
    let hub = hub().await;
    hub.set_webhooks(vec![signed_hook("pay")]);
    let secret = hub.tokens.create_secret("pay").unwrap().token;
    let body = r#"{"type":"invoice.paid"}"#;

    let (status, answer) = post_with(&hub, "pay", &[stripe_signature(&secret, body, now_secs())], body).await;
    assert_eq!(status, 202, "{answer}");
    hub.conversation_with("task-hook-pay", 2).await;

    // A call signed ten minutes ago is a replay; so is a fresh timestamp pasted over an old signature.
    let old = stripe_signature(&secret, body, now_secs() - 600);
    let moved = ("Stripe-Signature", format!("t={},v1={}", now_secs(), old.1.rsplit("v1=").next().unwrap()));
    let (a, b) = tokio::join!(post_with(&hub, "pay", &[old], body), post_with(&hub, "pay", &[moved], body));
    assert_eq!((a.0, b.0), (401, 401));
    assert_eq!(hub.inputs.lock().unwrap().len(), 1, "only the good call reached the model");
}

fn slack_signature(secret: &str, body: &str, at: i64) -> [(&'static str, String); 2] {
    [("X-Slack-Signature", format!("v0={}", sign(secret, format!("v0:{at}:{body}").as_bytes()))), ("X-Slack-Request-Timestamp", at.to_string())]
}

#[tokio::test]
async fn a_slack_signed_call_runs_and_a_stale_or_moved_timestamp_or_a_missing_time_is_refused() {
    let hub = hub().await;
    hub.set_webhooks(vec![signed_hook("slack")]);
    let secret = hub.tokens.create_secret("slack").unwrap().token;
    let body = "command=%2Fdeploy&text=prod";

    let (status, answer) = post_with(&hub, "slack", &slack_signature(&secret, body, now_secs()), body).await;
    assert_eq!(status, 202, "{answer}");
    hub.conversation_with("task-hook-slack", 2).await;

    let old = slack_signature(&secret, body, now_secs() - 600);
    let moved = [old[0].clone(), ("X-Slack-Request-Timestamp", now_secs().to_string())];
    let no_time = [slack_signature(&secret, body, now_secs())[0].clone()];
    let (a, b, c) = tokio::join!(post_with(&hub, "slack", &old, body), post_with(&hub, "slack", &moved, body), post_with(&hub, "slack", &no_time, body));
    assert_eq!((a.0, b.0, c.0), (401, 401, 401));
    assert_eq!(hub.inputs.lock().unwrap().len(), 1, "only the good call reached the model");
}

#[tokio::test]
async fn every_way_of_a_bad_signature_gets_the_same_401_as_an_unknown_webhook_and_runs_nothing() {
    let hub = hub().await;
    hub.set_webhooks(vec![signed_hook("gh")]);
    let secret = hub.tokens.create_secret("gh").unwrap().token;
    let body = r#"{"a":1}"#;

    let started = Instant::now();
    let (wrong_secret, tampered, unsigned, malformed, another_hook, unknown, both_bad) = tokio::join!(
        post_with(&hub, "gh", &[github_signature("another secret", body)], body),
        post_with(&hub, "gh", &[github_signature(&secret, body)], r#"{"a":2}"#),
        post_with(&hub, "gh", &[], body),
        post_with(&hub, "gh", &[("X-Hub-Signature-256", "sha256=not-hex".to_string())], body),
        post_with(&hub, "gh", &[("X-Hub-Signature-256", format!("sha1={}", sign(&secret, body.as_bytes())))], body),
        post_with(&hub, "ghost", &[github_signature(&secret, body)], body),
        // A good Stripe signature does not rescue a bad GitHub one.
        post_with(&hub, "gh", &[("X-Hub-Signature-256", "sha256=00".to_string()), stripe_signature(&secret, body, now_secs())], body),
    );
    for (status, answer) in [&wrong_secret, &tampered, &unsigned, &malformed, &another_hook, &unknown, &both_bad] {
        assert_eq!(*status, 401, "{answer}");
    }
    assert!(started.elapsed() >= Duration::from_millis(900), "a bad signature waits before it is told so");
    let first = &wrong_secret.1;
    assert!([&tampered, &unsigned, &malformed, &another_hook, &unknown, &both_bad].iter().all(|r| &r.1 == first), "the answer doesn't say which part was wrong");

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(hub.inputs.lock().unwrap().is_empty(), "the model was never called");
    assert!(hub.conversation("task-hook-gh").is_none(), "nothing was saved");
}

#[tokio::test]
async fn a_repeated_delivery_runs_once_and_a_forged_one_cannot_use_up_an_id() {
    let hub = hub().await;
    hub.set_webhooks(vec![signed_hook("gh")]);
    let secret = hub.tokens.create_secret("gh").unwrap().token;
    let body = r#"{"n":1}"#;
    let delivery = |id: &str| ("X-GitHub-Delivery", id.to_string());

    // Someone without the secret tries an id first: refused, and the id is not spent.
    let forged = post_with(&hub, "gh", &[("X-Hub-Signature-256", "sha256=00".to_string()), delivery("d-1")], body).await;
    assert_eq!(forged.0, 401);

    assert_eq!(post_with(&hub, "gh", &[github_signature(&secret, body), delivery("d-1")], body).await.0, 202);
    hub.conversation_with("task-hook-gh", 2).await;
    wait_until_free(&hub, &secret).await;

    // GitHub sends the same delivery again: acknowledged, not run. (`wait_until_free` made a call of its own, so what
    // counts is what was there just before the repeat.)
    let (messages, runs) = (hub.conversation("task-hook-gh").unwrap().messages.len(), hub.inputs.lock().unwrap().len());
    let (status, answer) = post_with(&hub, "gh", &[github_signature(&secret, body), delivery("d-1")], body).await;
    assert_eq!((status, json(&answer)["status"].as_str()), (202, Some("duplicate")));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(hub.conversation("task-hook-gh").unwrap().messages.len(), messages, "the repeat added nothing");
    assert_eq!(hub.inputs.lock().unwrap().len(), runs, "and the model wasn't called");

    // A new id runs, and so does a call that has no id at all (each adds two messages; the probe in between adds two).
    assert_eq!(post_with(&hub, "gh", &[github_signature(&secret, body), delivery("d-2")], body).await.0, 202);
    hub.conversation_with("task-hook-gh", messages + 2).await;
    wait_until_free(&hub, &secret).await;
    assert_eq!(post_with(&hub, "gh", &[github_signature(&secret, body)], body).await.0, 202);
    hub.conversation_with("task-hook-gh", messages + 6).await;
}

/// Waits until the webhook takes a call again (the running mark is dropped just after the conversation is saved), using a
/// delivery id nobody else uses so the call itself is never a duplicate. It leaves that call's run to finish.
async fn wait_until_free(hub: &Hub, secret: &str) {
    static PROBES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let before = hub.conversation("task-hook-gh").map_or(0, |c| c.messages.len());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // A counter, so no two probes ever share an id: the same id twice would be a repeat, answered without running.
        let probe = format!("probe-{}", PROBES.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        let (status, _) = post_with(hub, "gh", &[github_signature(secret, "p"), ("X-GitHub-Delivery", probe)], "p").await;
        if status == 202 {
            hub.conversation_with("task-hook-gh", before + 2).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            return;
        }
        assert!(Instant::now() < deadline, "the webhook never took a call again");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_credential_has_to_be_of_the_kind_the_webhook_wants() {
    let hub = hub().await;
    let body = "x";

    // A webhook that wants a signature, with a token made for it: the token proves nothing.
    hub.set_webhooks(vec![signed_hook("gh")]);
    let token = hub.tokens.create("gh").unwrap().token;
    assert_eq!(hub.post("gh", Some(&token), body).await.0, 401);
    // A secret then makes it work.
    let secret = hub.tokens.create_secret("gh").unwrap().token;
    assert_eq!(post_with(&hub, "gh", &[github_signature(&secret, body)], body).await.0, 202);
    hub.conversation_with("task-hook-gh", 2).await;

    // The other way: a webhook switched to tokens still holds a secret, and a correctly signed call is refused.
    hub.set_webhooks(vec![hook("gh", Some("poet"), "Why did the build fail?")]);
    assert_eq!(post_with(&hub, "gh", &[github_signature(&secret, body)], body).await.0, 401);
    assert_eq!(hub.post("gh", Some(&secret), body).await.0, 401);
    let token = hub.tokens.create("gh").unwrap().token;
    let deadline = Instant::now() + Duration::from_secs(10);
    while hub.post("gh", Some(&token), body).await.0 != 202 {
        assert!(Instant::now() < deadline, "the new token never worked");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
