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
use warden_bootstrap::webhooks::WebhookConfig;
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
        owner: None,
        shared_with: Vec::new(),
    }
}

fn hook(id: &str, agent: Option<&str>, prompt: &str) -> WebhookConfig {
    WebhookConfig { id: id.into(), agent: agent.map(str::to_string), prompt: prompt.into(), enabled: true }
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
