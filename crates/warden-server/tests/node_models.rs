//! P93, fatia 3: a node's own model answering the hub's agents. A `kind = "node"` provider streams
//! from the node (text in pieces, a tool call that runs on the hub, usage); an agent the node isn't
//! open to falls through its combo to a real `openai_compatible` spare; a node that drops mid-answer
//! fails the turn at once; an offline node is unavailable.
//!
//! One test, in order: the hub's model router is process-wide, so two hubs in one test binary would
//! race for it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use warden_bootstrap::{save_config, AgentConfig, ComboConfig, FileConfig, NodeAccessConfig, Provider, ProviderConfig};
use warden_core::memory::Vault;
use warden_core::model::{ChatStream, Message, ModelProvider, Role, StreamEvent, Usage};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::node_client::{serve_once, LocalNode, NodeIdentity, NodeSession};
use warden_server::{ClientMessage, PairingStore, Server, ServerConnection, ServerMessage, SettingsHost};

const NODE: &str = "node-models-1";

/// Answers every request with a whole HTTP/1.1 response — the combo's spare provider.
async fn fake_http(response: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{addr}/v1")
}

const SPARE: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"from the spare\"}}]}\n\ndata: [DONE]\n\n";

/// The node's local model: text in two pieces with usage; `TOOL` asks for `list_nodes`; a tool
/// result comes back as "node saw: …"; `SLOW` sends a piece and then hangs.
struct NodeModel;

#[async_trait]
impl ModelProvider for NodeModel {
    async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let last = messages.last().unwrap().clone();
        let events: Vec<StreamEvent> = if last.role == Role::Tool {
            vec![StreamEvent::ContentDelta(format!("node saw: {}", last.content))]
        } else if last.content.contains("TOOL") {
            vec![StreamEvent::ToolCallDelta { index: 0, id: Some("c1".into()), name: Some("list_nodes".into()), arguments_delta: Some("{}".into()), thought_signature: None }]
        } else if last.content.contains("SLOW") {
            let first = stream::iter(vec![Ok(StreamEvent::ContentDelta("partial".into()))]);
            let hang = stream::once(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(StreamEvent::ContentDelta("never".into()))
            });
            return Ok(Box::pin(futures_util::StreamExt::chain(first, hang)));
        } else {
            vec![
                StreamEvent::ContentDelta("from ".into()),
                StreamEvent::ContentDelta("the node".into()),
                StreamEvent::Usage(Usage { prompt_tokens: 5, completion_tokens: 3, total_tokens: 8 }),
            ]
        };
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

/// The hub's own default model — never used: every agent here has its own.
struct Unused;

#[async_trait]
impl ModelProvider for Unused {
    async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        anyhow::bail!("the hub's default model was used")
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
        anyhow::bail!("not used by this test")
    }
}

fn agent(id: &str, provider: &str) -> AgentConfig {
    AgentConfig {
        id: id.into(),
        persona: format!("You are {id}."),
        provider_id: Some(provider.into()),
        can_delegate_to_agents: false,
        can_manage_agents: false,
        can_message_agents: false,
        can_manage_tasks: false,
        allowed_tools: None,
    }
}

fn start_node(url: &str) -> tokio::task::JoinHandle<()> {
    let local = Arc::new(LocalNode::new(false, None).with_models(vec![("local".into(), Arc::new(NodeModel) as Arc<dyn ModelProvider>)]));
    let session = NodeSession { hub_url: url.into(), name: "Models Node".into(), auth_key: "test-key".into(), offer: local.offer(String::new(), vec![]), identity_path: None };
    tokio::spawn(async move {
        let mut identity = NodeIdentity { device_id: NODE.into(), device_token: None };
        let _ = serve_once(&session, &mut identity, local).await;
    })
}

/// Sends a chat turn as `agent` and returns the reply (or `error: …`) and how many fallbacks it reports.
async fn chat(conn: &mut ServerConnection, message: &str, agent: &str) -> (String, usize) {
    conn.send(&ClientMessage::Chat { message: message.into(), conversation_id: Some(format!("c-{agent}")), attachments: Vec::new(), agent_id: Some(agent.into()) })
        .await
        .unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            ServerMessage::ChatResponse { content, fallbacks, .. } => return (content, fallbacks.len()),
            ServerMessage::ChatError { message, .. } => return (format!("error: {message}"), 0),
            _ => continue,
        }
    }
}

async fn wait_online(conn: &mut ServerConnection, online: bool) {
    for _ in 0..100 {
        conn.send(&ClientMessage::ListNodes { request_id: 1 }).await.unwrap();
        loop {
            if let Some(ServerMessage::NodeList { nodes, .. }) = conn.recv().await.unwrap() {
                if nodes.iter().any(|n| n.device_id == NODE && n.online == online) {
                    return;
                }
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the node never became online={online}");
}

#[tokio::test]
async fn a_nodes_model_answers_the_hubs_agents() {
    let dir = std::env::temp_dir().join(format!("warden-server-node-models-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    let spare_url = fake_http(SPARE).await;
    let config = FileConfig {
        providers: vec![
            ProviderConfig { id: "casa".into(), kind: Provider::Node, api_key: None, base_url: None, model: Some("local".into()), node: Some(NODE.into()) },
            ProviderConfig { id: "spare".into(), kind: Provider::OpenaiCompatible, api_key: Some("x".into()), base_url: Some(spare_url), model: Some("spare-model".into()), node: None },
        ],
        combos: vec![ComboConfig { id: "casa-then-spare".into(), providers: vec!["casa".into(), "spare".into()] }],
        agents: vec![agent("ops", "casa"), agent("other", "casa-then-spare")],
        nodes: vec![NodeAccessConfig { id: NODE.into(), enabled: true, agents: vec!["ops".into()], require_approval: false }],
        ..FileConfig::default()
    };
    save_config(&config_path, &config).unwrap();

    let orchestrator = Orchestrator::new(Arc::new(Unused), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(TestHost { path: config_path }))
        .with_node_audit(None);
    let url = format!("ws://{}", server.local_addr().unwrap());
    tokio::spawn(server.serve());

    let mut web = ServerConnection::connect(&url, "web-1", "Browser", "test-key").await.unwrap();
    let node = start_node(&url);
    wait_online(&mut web, true).await;

    // Not approved yet: the node's model is out of reach.
    let (reply, _) = chat(&mut web, "hello", "ops").await;
    assert!(reply.starts_with("error:") && reply.contains("approved"), "{reply}");
    PairingStore::new(dir.join("devices.json")).approve(NODE).unwrap();

    // Streamed from the node, in pieces.
    let (reply, fallbacks) = chat(&mut web, "hello", "ops").await;
    assert_eq!((reply.as_str(), fallbacks), ("from the node", 0));

    // The node's model asks for a tool; it runs on the hub and the result goes back to the node.
    let (reply, _) = chat(&mut web, "TOOL please", "ops").await;
    assert!(reply.starts_with("node saw:") && reply.contains(NODE), "{reply}");

    // An agent the node isn't open to: its combo moves on to the spare.
    let (reply, fallbacks) = chat(&mut web, "hello", "other").await;
    assert_eq!((reply.as_str(), fallbacks), ("from the spare", 1));

    // The node drops in the middle of an answer: the turn fails now, not after 30 s.
    let started = std::time::Instant::now();
    let url2 = url.clone();
    let turn = tokio::spawn(async move {
        let mut conn = ServerConnection::connect(&url2, "web-2", "Browser 2", "test-key").await.unwrap();
        chat(&mut conn, "SLOW please", "ops").await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    node.abort();
    let (reply, _) = tokio::time::timeout(Duration::from_secs(10), turn).await.expect("the turn waited out the node").unwrap();
    assert!(reply.starts_with("error:") && reply.contains("disconnected in the middle of the answer"), "{reply}");
    assert!(started.elapsed() < Duration::from_secs(10));

    // Offline: out of reach for `ops`, and the combo still answers `other`.
    wait_online(&mut web, false).await;
    let (reply, _) = chat(&mut web, "hello", "ops").await;
    assert!(reply.starts_with("error:") && reply.contains("offline"), "{reply}");
    let (reply, fallbacks) = chat(&mut web, "hello", "other").await;
    assert_eq!((reply.as_str(), fallbacks), ("from the spare", 1));
}
