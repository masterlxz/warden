//! P12 — the Warden API over a real socket: a real `Server` with a scripted model, spoken to with
//! plain HTTP the way an OpenAI SDK would, next to the web UI and the WebSocket protocol on the
//! same port.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use warden_core::memory::Vault;
use warden_core::model::{ChatStream, Message, ModelProvider, Role, StreamEvent, Usage};
use warden_core::orchestrator::Orchestrator;
use warden_core::spend::{MemoryStore, PriceTable, SpendGuard};
use warden_core::tool::ToolSpec;
use warden_server::api_keys::ApiKeyStore;
use warden_server::{Server, ServerConnection, SettingsHost, StaticWebUi};

/// Answers "echo: <last user message>" in two pieces, with usage, and keeps every system prompt it saw.
struct Echo {
    systems: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ModelProvider for Echo {
    async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let system = messages.iter().filter(|m| m.role == Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
        self.systems.lock().unwrap().push(system);
        let last = messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default();
        let history = messages.iter().filter(|m| m.role == Role::User).count();
        let events = vec![
            Ok(StreamEvent::ContentDelta("echo: ".into())),
            Ok(StreamEvent::ContentDelta(format!("{last} ({history} user messages)"))),
            Ok(StreamEvent::Usage(Usage { prompt_tokens: 7, completion_tokens: 3, total_tokens: 10 })),
        ];
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
        anyhow::bail!("not used by this test")
    }
}

struct Hub {
    addr: SocketAddr,
    key: String,
    keys: ApiKeyStore,
    systems: Arc<Mutex<Vec<String>>>,
    guard: Arc<SpendGuard>,
    config: PathBuf,
}

async fn hub() -> Hub {
    let dir = std::env::temp_dir().join(format!("warden-openai-api-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(&config, "[[agents]]\nid = \"poet\"\npersona = \"You are the poet.\"\n").unwrap();
    let systems = Arc::new(Mutex::new(Vec::new()));
    let guard = Arc::new(SpendGuard::new(Arc::new(MemoryStore::default()), Vec::new(), PriceTable::new(Vec::new())));
    let orchestrator = Orchestrator::new(Arc::new(Echo { systems: systems.clone() }), Arc::new(Vault::new(dir.join("vault")))).with_spend_guard(guard.clone());
    let keys = ApiKeyStore::new(dir.join("api_keys.json"));
    let key = keys.create("script", None).unwrap().key;
    let web = StaticWebUi([("index.html".to_string(), b"<p>web</p>".to_vec())].into_iter().collect());
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "pairing-key-0123456789-0123456789", "Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(Host { path: config.clone() }))
        .with_web_ui(Arc::new(web))
        .with_api(dir.join("api_keys.json"));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { addr, key, keys, systems, guard, config }
}

/// One request, the whole answer: (status code, body).
async fn http(addr: SocketAddr, method: &str, path: &str, key: Option<&str>, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n");
    if let Some(key) = key {
        request.push_str(&format!("Authorization: Bearer {key}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body.unwrap_or_default());
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, body.to_string())
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
}

#[tokio::test]
async fn models_lists_the_hub_and_its_agents_and_a_wrong_key_is_refused() {
    let hub = hub().await;
    let (status, body) = http(hub.addr, "GET", "/v1/models", Some(&hub.key), None).await;
    assert_eq!(status, 200, "{body}");
    let ids: Vec<String> = json(&body)["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["warden", "warden/poet"]);

    let started = std::time::Instant::now();
    let (status, body) = http(hub.addr, "GET", "/v1/models", Some("wdn_wrong"), None).await;
    assert_eq!(status, 401);
    assert_eq!(json(&body)["error"]["code"], "invalid_api_key");
    assert!(started.elapsed() >= std::time::Duration::from_secs(1), "a wrong key waits");
    assert_eq!(http(hub.addr, "GET", "/v1/models", None, None).await.0, 401);

    let id = hub.keys.list().unwrap()[0].id.clone();
    hub.keys.revoke(&id).unwrap();
    assert_eq!(http(hub.addr, "GET", "/v1/models", Some(&hub.key), None).await.0, 401, "a revoked key stops at once");
}

#[tokio::test]
async fn a_completion_speaks_as_the_agent_with_the_clients_system_text_and_is_billed_to_api() {
    let hub = hub().await;
    let body = r#"{"model":"warden/poet","messages":[{"role":"system","content":"Answer in rhyme."},{"role":"user","content":"hi"},{"role":"assistant","content":"hello"},{"role":"user","content":"again"}],"tools":[{"type":"function","function":{"name":"ignored","parameters":{}}}]}"#;
    let (status, reply) = http(hub.addr, "POST", "/v1/chat/completions", Some(&hub.key), Some(body)).await;
    assert_eq!(status, 200, "{reply}");
    let reply = json(&reply);
    assert_eq!(reply["object"], "chat.completion");
    assert_eq!(reply["model"], "warden/poet");
    assert_eq!(reply["choices"][0]["message"]["content"], "echo: again (2 user messages)");
    assert_eq!(reply["usage"]["total_tokens"], 10);
    let system = hub.systems.lock().unwrap().last().cloned().unwrap();
    assert!(system.contains("You are the poet.") && system.contains("Answer in rhyme."), "{system}");
    assert_eq!(hub.guard.spent_since("api", 0).calls, 1, "the spend goes to the api channel");
}

#[tokio::test]
async fn a_streamed_completion_arrives_in_chunks_then_done() {
    let hub = hub().await;
    let body = r#"{"model":"warden","stream":true,"stream_options":{"include_usage":true},"messages":[{"role":"user","content":"hi"}]}"#;
    let (status, reply) = http(hub.addr, "POST", "/v1/chat/completions", Some(&hub.key), Some(body)).await;
    assert_eq!(status, 200, "{reply}");
    let events: Vec<&str> = reply.split("\n\n").filter_map(|e| e.strip_prefix("data: ")).collect();
    assert_eq!(events.last(), Some(&"[DONE]"));
    let chunks: Vec<serde_json::Value> = events[..events.len() - 1].iter().map(|e| json(e)).collect();
    let text: String = chunks.iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
    assert_eq!(text, "echo: hi (1 user messages)");
    assert!(chunks.iter().any(|c| c["choices"][0]["finish_reason"] == "stop"));
    assert_eq!(chunks.last().unwrap()["usage"]["total_tokens"], 10);
}

#[tokio::test]
async fn bad_requests_get_openai_errors() {
    let hub = hub().await;
    let post = |body: &'static str| {
        let (addr, key) = (hub.addr, hub.key.clone());
        async move { http(addr, "POST", "/v1/chat/completions", Some(&key), Some(body)).await }
    };
    let (status, body) = post(r#"{"model":"warden/nobody","messages":[{"role":"user","content":"hi"}]}"#).await;
    assert_eq!((status, json(&body)["error"]["code"].as_str()), (404, Some("model_not_found")));
    let (status, body) = post(r#"{"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:x"}}]}]}"#).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(post("not json").await.0, 400);
    assert_eq!(post(r#"{"messages":[{"role":"assistant","content":"x"}]}"#).await.0, 400);
    assert_eq!(http(hub.addr, "GET", "/v1/chat/completions", Some(&hub.key), None).await.0, 405);
    assert_eq!(http(hub.addr, "GET", "/v1/embeddings", Some(&hub.key), None).await.0, 404);
}

#[tokio::test]
async fn the_web_page_and_the_websocket_still_answer_on_the_same_port() {
    let hub = hub().await;
    let (status, body) = http(hub.addr, "GET", "/", None, None).await;
    assert_eq!((status, body.as_str()), (200, "<p>web</p>"));
    ServerConnection::connect(&format!("ws://{}", hub.addr), "phone", "Phone", "pairing-key-0123456789-0123456789").await.unwrap();
}

#[tokio::test]
async fn a_key_bound_to_an_agent_only_speaks_as_it() {
    let hub = hub().await;
    std::fs::write(&hub.config, "[[agents]]\nid = \"poet\"\npersona = \"You are the poet.\"\n\n[[agents]]\nid = \"admin\"\npersona = \"You are the admin.\"\n").unwrap();
    let bound = hub.keys.create("bot", Some("poet")).unwrap().key;

    let (_, body) = http(hub.addr, "GET", "/v1/models", Some(&bound), None).await;
    let ids: Vec<String> = json(&body)["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["warden/poet"], "only its agent");

    for body in [r#"{"model":"warden","messages":[{"role":"user","content":"hi"}]}"#, r#"{"messages":[{"role":"user","content":"hi"}]}"#] {
        let (status, reply) = http(hub.addr, "POST", "/v1/chat/completions", Some(&bound), Some(body)).await;
        assert_eq!(status, 200, "{reply}");
        assert_eq!(json(&reply)["model"], "warden/poet");
        assert!(hub.systems.lock().unwrap().last().unwrap().contains("You are the poet."));
    }
    let (status, reply) = http(hub.addr, "POST", "/v1/chat/completions", Some(&bound), Some(r#"{"model":"warden/admin","messages":[{"role":"user","content":"hi"}]}"#)).await;
    assert_eq!((status, json(&reply)["error"]["code"].as_str()), (403, Some("model_not_allowed")));

    // The general key still picks freely.
    let (status, reply) = http(hub.addr, "POST", "/v1/chat/completions", Some(&hub.key), Some(r#"{"model":"warden/admin","messages":[{"role":"user","content":"hi"}]}"#)).await;
    assert_eq!(status, 200, "{reply}");
    assert!(hub.systems.lock().unwrap().last().unwrap().contains("You are the admin."));

    // The agent is gone: refused, never the hub's default instead.
    std::fs::write(&hub.config, "[[agents]]\nid = \"admin\"\npersona = \"You are the admin.\"\n").unwrap();
    let (status, reply) = http(hub.addr, "POST", "/v1/chat/completions", Some(&bound), Some(r#"{"messages":[{"role":"user","content":"hi"}]}"#)).await;
    assert_eq!((status, json(&reply)["error"]["code"].as_str()), (403, Some("agent_gone")), "{reply}");
}
