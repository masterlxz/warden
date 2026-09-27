//! P93 on the hub: a real node (`node_client::serve_once`) joins a real `Server`, and the hub's agents
//! use its shell and files through `list_nodes`/`node_shell`/`node_write_file`/`node_read_file` — only
//! once it's approved and allowed, only for the agents allowed, with a yes when it asks for one, and
//! a call cut short when the node drops.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use warden_bootstrap::{save_config, AgentConfig, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::node_client::{run_node, serve_once, LocalNode, NodeActivity, NodeIdentity, NodeSession, NodeState};
use warden_server::{ClientMessage, PairingStore, Server, ServerConnection, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::NodeInfoDto;

const NODE: &str = "node-test-1";

/// The tools each model call was offered.
type Offered = Arc<Mutex<Vec<Vec<String>>>>;

/// `RUN`, `WRITE`, `READ`, `SLEEP` ask for the matching node tool; a tool result comes back as
/// "tool said: …".
struct Scripted {
    offered: Offered,
}

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        self.offered.lock().unwrap().push(tools.iter().map(|t| t.name.clone()).collect());
        let last = messages.last().unwrap();
        if last.role == Role::Tool {
            return Ok(response_stream(Response { content: format!("tool said: {}", last.content), tool_calls: Vec::new(), usage: None }));
        }
        let call = |name: &str, arguments: serde_json::Value| {
            Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall { id: "call-1".into(), name: name.into(), arguments, thought_signature: None }],
                usage: None,
            }))
        };
        let text = last.content.as_str();
        if text.contains("RUN") {
            return call("node_shell", json!({ "node": NODE, "command": "echo from-the-node" }));
        }
        if text.contains("WRITE") {
            return call("node_write_file", json!({ "node": NODE, "path": "notes/hello.txt", "content": "written by the hub" }));
        }
        if text.contains("READ") {
            return call("node_read_file", json!({ "node": NODE, "path": "notes/hello.txt" }));
        }
        if text.contains("MCP") {
            return call("test-node__echo_text", json!({ "text": "hi from the hub" }));
        }
        if text.contains("SLEEP") {
            return call("node_shell", json!({ "node": NODE, "command": "sleep 30", "timeout_ms": 60000 }));
        }
        Ok(response_stream(Response { content: "plain".into(), tool_calls: Vec::new(), usage: None }))
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

fn agent(id: &str) -> AgentConfig {
    AgentConfig {
        id: id.into(),
        persona: format!("You are {id}."),
        provider_id: None,
        can_delegate_to_agents: false,
        can_manage_agents: false,
        can_message_agents: false,
        can_manage_tasks: false,
        allowed_tools: None,
    }
}

struct Hub {
    url: String,
    dir: PathBuf,
    offered: Offered,
}

async fn spin_up() -> Hub {
    let dir = std::env::temp_dir().join(format!(
        "warden-server-nodes-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(dir.join("shared")).unwrap();
    let config_path = dir.join("config.toml");
    save_config(&config_path, &FileConfig { agents: vec![agent("ops"), agent("other")], ..FileConfig::default() }).unwrap();

    let offered: Offered = Arc::default();
    let orchestrator = Orchestrator::new(Arc::new(Scripted { offered: offered.clone() }), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(TestHost { path: config_path }))
        .with_node_audit(Some(dir.join("node_audit.jsonl")));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}"), dir, offered }
}

/// Stands in for a tool of an MCP server on the node: echoes its `text`.
struct EchoText;

#[async_trait]
impl warden_core::tool::Tool for EchoText {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo_text".into(),
            description: "Echoes the text back".into(),
            parameters: json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
        }
    }

    async fn call(&self, args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Ok(json!({ "echoed": args["text"] }))
    }
}

/// Starts the node in the background; aborting the handle drops its connection.
fn start_node(hub: &Hub) -> tokio::task::JoinHandle<()> {
    let local = Arc::new(LocalNode::new(true, Some(hub.dir.join("shared"))).with_mcp_tools(vec![Arc::new(EchoText)]));
    let session = NodeSession {
        hub_url: hub.url.clone(),
        name: "Test Node".into(),
        auth_key: "test-key".into(),
        offer: local.offer("the test machine".into(), vec!["test".into()]),
        identity_path: None,
    };
    tokio::spawn(async move {
        let mut identity = NodeIdentity { device_id: NODE.into(), device_token: None };
        let _ = serve_once(&session, &mut identity, local).await;
    })
}

async fn nodes(conn: &mut ServerConnection) -> Vec<NodeInfoDto> {
    conn.send(&ClientMessage::ListNodes { request_id: 1 }).await.unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            ServerMessage::NodeList { nodes, .. } => return nodes,
            _ => continue,
        }
    }
}

async fn wait_online(conn: &mut ServerConnection, online: bool) {
    for _ in 0..100 {
        if nodes(conn).await.iter().any(|n| n.device_id == NODE && n.online == online) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the node never became online={online}");
}

fn set_access(key: &str, agents: &[&str], require_approval: bool) -> ClientMessage {
    ClientMessage::SetNodeAccess {
        request_id: 2,
        pairing_key: key.into(),
        device_id: NODE.into(),
        enabled: true,
        agents: agents.iter().map(|a| a.to_string()).collect(),
        require_approval,
    }
}

async fn node_reply(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            msg @ (ServerMessage::NodeList { .. } | ServerMessage::NodeError { .. }) => return msg,
            _ => continue,
        }
    }
}

/// Sends a chat turn as `agent` and returns the reply text, answering approvals with `approve`.
async fn chat(conn: &mut ServerConnection, message: &str, agent: &str, approve: bool) -> (String, usize) {
    conn.send(&ClientMessage::Chat { message: message.into(), conversation_id: Some(format!("c-{agent}")), attachments: Vec::new(), agent_id: Some(agent.into()) })
        .await
        .unwrap();
    let mut asked = 0;
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            ServerMessage::ApprovalRequest { approval_id, .. } => {
                asked += 1;
                conn.send(&ClientMessage::ResolveApproval { approval_id, approved: approve }).await.unwrap();
            }
            ServerMessage::ChatResponse { content, .. } => return (content, asked),
            ServerMessage::ChatError { message, .. } => return (format!("error: {message}"), asked),
            _ => continue,
        }
    }
}

fn last_offered(hub: &Hub) -> Vec<String> {
    hub.offered.lock().unwrap().last().cloned().unwrap_or_default()
}

#[tokio::test]
async fn agents_use_a_node_only_once_it_is_approved_and_allowed() {
    let hub = spin_up().await;
    let mut web = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    let _node = start_node(&hub);
    wait_online(&mut web, true).await;

    let listed = nodes(&mut web).await;
    let node = listed.iter().find(|n| n.device_id == NODE).unwrap();
    assert_eq!((node.name.as_str(), node.approved, node.enabled), ("Test Node", false, false));
    let offer = node.offer.clone().unwrap();
    assert!(offer.shell && offer.files && offer.tags == ["test"]);

    // Connected but not approved nor allowed: no node tools at all.
    chat(&mut web, "hello", "ops", false).await;
    assert!(!last_offered(&hub).iter().any(|t| t.starts_with("node_") || t == "list_nodes"));

    web.send(&set_access("wrong", &["ops"], false)).await.unwrap();
    assert!(matches!(node_reply(&mut web).await, ServerMessage::NodeError { auth_rejected: true, .. }));
    web.send(&set_access("test-key", &["ops"], false)).await.unwrap();
    assert!(matches!(node_reply(&mut web).await, ServerMessage::NodeList { .. }));
    // Allowed but not approved as a device yet: still nothing.
    chat(&mut web, "hello", "ops", false).await;
    assert!(!last_offered(&hub).iter().any(|t| t == "node_shell"));

    PairingStore::new(hub.dir.join("devices.json")).approve(NODE).unwrap();
    let (reply, _) = chat(&mut web, "RUN it", "ops", false).await;
    assert!(reply.contains("from-the-node"), "{reply}");
    assert!(last_offered(&hub).iter().any(|t| t == "list_nodes"));

    let (reply, _) = chat(&mut web, "WRITE it", "ops", false).await;
    assert!(reply.contains("ok"), "{reply}");
    assert_eq!(std::fs::read_to_string(hub.dir.join("shared/notes/hello.txt")).unwrap(), "written by the hub");
    let (reply, _) = chat(&mut web, "READ it", "ops", false).await;
    assert!(reply.contains("written by the hub"), "{reply}");

    // Another agent isn't on the node's list.
    chat(&mut web, "hello", "other", false).await;
    assert!(!last_offered(&hub).iter().any(|t| t.starts_with("node_")));

    // Asking first: the chatting device gets the card; a no runs nothing.
    web.send(&set_access("test-key", &["ops"], true)).await.unwrap();
    node_reply(&mut web).await;
    let (reply, asked) = chat(&mut web, "RUN it", "ops", false).await;
    assert_eq!(asked, 1);
    assert!(reply.contains("did not approve"), "{reply}");
    let (reply, asked) = chat(&mut web, "RUN it", "ops", true).await;
    assert_eq!(asked, 1);
    assert!(reply.contains("from-the-node"), "{reply}");

    let audit = std::fs::read_to_string(hub.dir.join("node_audit.jsonl")).unwrap();
    assert!(audit.lines().count() >= 4 && audit.contains("\"op\":\"node_write_file\"") && audit.contains("characters"), "{audit}");
    assert!(!audit.contains("written by the hub"), "file contents stay out of the log");
}

#[tokio::test]
async fn a_node_that_drops_mid_command_fails_the_call_at_once() {
    let hub = spin_up().await;
    let mut web = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    let node = start_node(&hub);
    wait_online(&mut web, true).await;
    PairingStore::new(hub.dir.join("devices.json")).approve(NODE).unwrap();
    web.send(&set_access("test-key", &[], false)).await.unwrap();
    node_reply(&mut web).await;

    let started = std::time::Instant::now();
    let turn = tokio::spawn(async move { chat(&mut web, "SLEEP", "ops", false).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    node.abort();
    let (reply, _) = tokio::time::timeout(Duration::from_secs(10), turn).await.expect("the call waited for its timeout").unwrap();
    assert!(reply.contains("disconnected in the middle of the call") && reply.contains("not retried"), "{reply}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn a_nodes_mcp_tools_come_and_go_with_it() {
    let hub = spin_up().await;
    let mut web = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    let node = start_node(&hub);
    wait_online(&mut web, true).await;
    let offer = nodes(&mut web).await.into_iter().find(|n| n.device_id == NODE).unwrap().offer.unwrap();
    assert_eq!(offer.mcp_tools[0].name, "echo_text");

    // Nothing before it's approved and allowed.
    chat(&mut web, "hello", "ops", false).await;
    assert!(!last_offered(&hub).iter().any(|t| t == "test-node__echo_text"));
    PairingStore::new(hub.dir.join("devices.json")).approve(NODE).unwrap();
    web.send(&set_access("test-key", &["ops"], false)).await.unwrap();
    node_reply(&mut web).await;

    let (reply, _) = chat(&mut web, "MCP please", "ops", false).await;
    assert!(reply.contains("hi from the hub"), "{reply}");
    assert!(last_offered(&hub).iter().any(|t| t == "test-node__echo_text"));
    chat(&mut web, "hello", "other", false).await;
    assert!(!last_offered(&hub).iter().any(|t| t == "test-node__echo_text"), "not for an agent off the list");

    web.send(&set_access("test-key", &["ops"], true)).await.unwrap();
    node_reply(&mut web).await;
    let (reply, asked) = chat(&mut web, "MCP please", "ops", true).await;
    assert_eq!(asked, 1);
    assert!(reply.contains("hi from the hub"), "{reply}");

    // The node leaves: its tool leaves with it.
    node.abort();
    wait_online(&mut web, false).await;
    chat(&mut web, "hello", "ops", false).await;
    assert!(!last_offered(&hub).iter().any(|t| t == "test-node__echo_text"));
    assert!(!last_offered(&hub).iter().any(|t| t == "list_nodes"), "and so do the node tools, with no node left");
}

/// P97: the loop the desktop runs (`run_node`, watched) — its state follows the connection and every
/// call the hub makes lands in the activity log.
#[tokio::test]
async fn a_watched_node_reports_its_state_and_what_agents_did() {
    let hub = spin_up().await;
    let mut web = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    let activity = NodeActivity::default();
    let local = Arc::new(LocalNode::new(true, Some(hub.dir.join("shared"))).with_activity(activity.clone()));
    let session = NodeSession { hub_url: hub.url.clone(), name: "Desk".into(), auth_key: "test-key".into(), offer: local.offer(String::new(), Vec::new()), identity_path: None };
    let (status, mut watching) = tokio::sync::watch::channel(NodeState::Connecting);
    let identity = NodeIdentity { device_id: NODE.into(), device_token: None };
    let node = tokio::spawn(run_node(session, identity, local, Some(status)));
    tokio::time::timeout(Duration::from_secs(10), watching.wait_for(|s| *s == NodeState::Connected)).await.expect("never connected").unwrap();

    wait_online(&mut web, true).await;
    PairingStore::new(hub.dir.join("devices.json")).approve(NODE).unwrap();
    web.send(&set_access("test-key", &[], false)).await.unwrap();
    node_reply(&mut web).await;
    let (reply, _) = chat(&mut web, "RUN it", "ops", false).await;
    assert!(reply.contains("from-the-node"), "{reply}");
    let log = activity.entries();
    assert_eq!((log[0].kind.as_str(), log[0].summary.as_str(), log[0].error.as_deref()), ("shell", "echo from-the-node", None));
    node.abort();
}

/// A wrong pairing key isn't something retrying fixes: `run_node` stops and says so, instead of
/// knocking on the hub forever.
#[tokio::test]
async fn a_node_turned_away_stops_instead_of_retrying() {
    let hub = spin_up().await;
    let local = Arc::new(LocalNode::new(true, None));
    let session = NodeSession { hub_url: hub.url.clone(), name: "Desk".into(), auth_key: "not-the-key".into(), offer: local.offer(String::new(), Vec::new()), identity_path: None };
    let (status, watching) = tokio::sync::watch::channel(NodeState::Connecting);
    let identity = NodeIdentity { device_id: NODE.into(), device_token: None };
    let result = tokio::time::timeout(Duration::from_secs(10), run_node(session, identity, local, Some(status))).await.expect("kept retrying");
    assert!(result.is_err());
    assert!(matches!(&*watching.borrow(), NodeState::Stopped { error } if error.contains("authentication rejected")), "{:?}", *watching.borrow());
}
