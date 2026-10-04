//! P102 fatia 2 on a real hub with a real node (`node_client::serve_once`): a conversation works in a folder on the node,
//! written `node:<id>:<path>` (the path inside what the node lends). The owner browses the node's folders and works in
//! one — files written and read there, a shell command only after a yes — and a member only inside the folders of that
//! node the owner named for them, on every turn. The model is a script: what it asks for is what is proved.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use warden_bootstrap::users::NodeFolder;
use warden_bootstrap::{load_config_from_path, save_config, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
use warden_core::tool::shell::ShellTool;
use warden_core::tool::ToolSpec;
use warden_server::node_client::{serve_once, LocalNode, NodeIdentity, NodeSession};
use warden_server::{ClientMessage, PairingStore, Server, ServerConnection, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::{DirEntryDto, NodeInfoDto};

const KEY: &str = "test-key";
const NODE: &str = "node-test-1";
const TEMP: &str = "provisional-1";

/// "WRITE" writes `out.txt`, "READ" reads it, "ESCAPE" tries to write above the folder, "RUN" runs a command in the
/// folder; a tool result ends the turn as "tool said: …". Anything else is "plain", with the briefing it was told.
struct Scripted;

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let last = messages.last().unwrap();
        if last.role == Role::Tool {
            return Ok(response_stream(Response { content: format!("tool said: {}", last.content), tool_calls: Vec::new(), usage: None }));
        }
        let call = |name: &str, arguments: serde_json::Value| {
            Ok(response_stream(Response { content: String::new(), tool_calls: vec![ToolCall { id: "call-1".into(), name: name.into(), arguments, thought_signature: None }], usage: None }))
        };
        match last.content.as_str() {
            "WRITE" => call("write_file", json!({ "path": "out.txt", "content": "from the hub" })),
            "READ" => call("read_file", json!({ "path": "out.txt" })),
            "ESCAPE" => call("write_file", json!({ "path": "../escaped.txt", "content": "x" })),
            "RUN" => call("shell", json!({ "command": "echo ran > made.txt" })),
            _ => {
                let told = messages.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("|");
                Ok(response_stream(Response { content: format!("plain: {told}"), tool_calls: Vec::new(), usage: None }))
            }
        }
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

struct Hub {
    url: String,
    dir: PathBuf,
    config_path: PathBuf,
}

impl Hub {
    /// The folder the node lends: `proj/sub`, `other`, and a file at the top.
    fn shared(&self) -> PathBuf {
        self.dir.join("shared")
    }
}

/// A hub with member Ana, who may work in `proj` of the node (and nothing else on it).
async fn spin_up() -> Hub {
    std::env::set_var(warden_core::memory::embed::OFF_SWITCH, "1");
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("warden-server-nodefolder-{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    for sub in ["shared/proj/sub", "shared/other"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("shared/top.txt"), "x").unwrap();
    let config_path = dir.join("config.toml");
    let mut config = FileConfig::default();
    warden_bootstrap::users::add_user(&mut config, "ana", "Ana", TEMP).unwrap();
    config.users[0].node_workdirs = vec![NodeFolder { node: NODE.into(), path: "proj".into() }];
    save_config(&config_path, &config).unwrap();

    let vault = Arc::new(Vault::new(dir.join("vault")));
    let mut orchestrator = Orchestrator::new(Arc::new(Scripted), vault.clone());
    orchestrator.register_tool(Arc::new(ReadFileTool::new(vault.clone())));
    orchestrator.register_tool(Arc::new(WriteFileTool::new(vault.clone())));
    orchestrator.register_tool(Arc::new(ShellTool::new(vault)));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_users_dir(dir.join("users"))
        .with_settings(Arc::new(Host { path: config_path.clone() }))
        .with_node_audit(Some(dir.join("node_audit.jsonl")));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}"), dir, config_path }
}

fn start_node(hub: &Hub) -> tokio::task::JoinHandle<()> {
    let local = Arc::new(LocalNode::new(true, Some(hub.shared())));
    let session = NodeSession { hub_url: hub.url.clone(), name: "Test Node".into(), auth_key: KEY.into(), offer: local.offer("the test machine".into(), Vec::new()), identity_path: None };
    tokio::spawn(async move {
        let mut identity = NodeIdentity { device_id: NODE.into(), device_token: None };
        let _ = serve_once(&session, &mut identity, local).await;
    })
}

async fn next(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), conn.recv()).await.expect("no message").unwrap().expect("connection closed") {
            ServerMessage::ConversationsChanged { .. } | ServerMessage::Pong { .. } => continue,
            msg => return msg,
        }
    }
}

async fn nodes(conn: &mut ServerConnection) -> Vec<NodeInfoDto> {
    conn.send(&ClientMessage::ListNodes { request_id: 1 }).await.unwrap();
    loop {
        if let ServerMessage::NodeList { nodes, .. } = next(conn).await {
            return nodes;
        }
    }
}

/// The owner, with the node online, approved and switched on for every agent, asking nothing by itself.
async fn owner_with_node(hub: &Hub) -> (ServerConnection, tokio::task::JoinHandle<()>) {
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let node = start_node(hub);
    for _ in 0..100 {
        if nodes(&mut owner).await.iter().any(|n| n.device_id == NODE && n.online) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    PairingStore::new(hub.dir.join("devices.json")).approve(NODE).unwrap();
    owner.send(&ClientMessage::SetNodeAccess { request_id: 2, pairing_key: KEY.into(), device_id: NODE.into(), enabled: true, agents: Vec::new(), require_approval: false }).await.unwrap();
    loop {
        if matches!(next(&mut owner).await, ServerMessage::NodeList { .. } | ServerMessage::NodeError { .. }) {
            break;
        }
    }
    (owner, node)
}

async fn ana(hub: &Hub) -> ServerConnection {
    let (mut conn, _, _) = ServerConnection::handshake_as_member(&hub.url, "ana-phone", "Ana's phone", "ana", TEMP, None, warden_server_protocol::tls::default_client_config()).await.unwrap();
    conn.send(&ClientMessage::ChangePassword { request_id: 1, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(next(&mut conn).await, ServerMessage::PasswordChanged { .. }));
    conn
}

fn folder(path: &str) -> String {
    format!("node:{NODE}:{path}")
}

async fn dirs(conn: &mut ServerConnection, path: Option<&str>) -> Result<(String, Option<String>, Vec<DirEntryDto>), String> {
    conn.send(&ClientMessage::ListDirs { request_id: 7, path: path.map(str::to_string) }).await.unwrap();
    match next(conn).await {
        ServerMessage::DirList { request_id: 7, path, parent, dirs } => Ok((path, parent, dirs)),
        ServerMessage::DirError { request_id: 7, message } => Err(message),
        other => panic!("{other:?}"),
    }
}

/// Sends a turn in `conversation` and answers what the turn asks: `approve` is the answer to a shell command's yes.
/// Returns the reply (`error: …` for a `ChatError`) and how many times the person was asked.
async fn say(conn: &mut ServerConnection, conversation: &str, text: &str, workdir: Option<&str>, approve: bool) -> (String, usize) {
    conn.send(&ClientMessage::Chat { message: text.into(), conversation_id: Some(conversation.into()), attachments: Vec::new(), agent_id: None, project_id: None, workdir: workdir.map(str::to_string) }).await.unwrap();
    let mut asked = 0;
    loop {
        match next(conn).await {
            ServerMessage::ApprovalRequest { approval_id, .. } => {
                asked += 1;
                conn.send(&ClientMessage::ResolveApproval { approval_id, approved: approve, always: false }).await.unwrap();
            }
            ServerMessage::ChatResponse { content, .. } => return (content, asked),
            ServerMessage::ChatError { message, .. } => return (format!("error: {message}"), asked),
            _ => {}
        }
    }
}

async fn workdir_of(conn: &mut ServerConnection, id: &str) -> Option<String> {
    conn.send(&ClientMessage::ListConversations { request_id: 9 }).await.unwrap();
    match next(conn).await {
        ServerMessage::ConversationList { conversations, .. } => conversations.into_iter().find(|c| c.id == id).unwrap().workdir,
        other => panic!("{other:?}"),
    }
}

fn names(dirs: &[DirEntryDto]) -> Vec<&str> {
    dirs.iter().map(|d| d.name.as_str()).collect()
}

#[tokio::test]
async fn the_owner_browses_a_nodes_folders_and_works_in_one() {
    let hub = spin_up().await;
    let (mut me, _node) = owner_with_node(&hub).await;

    // The browser: the folders of what the node lends, as node references, with the way up.
    let (path, parent, listed) = dirs(&mut me, Some(&folder(""))).await.unwrap();
    assert_eq!((path.as_str(), parent), (folder("").as_str(), None));
    assert_eq!(names(&listed), ["other", "proj"], "folders only: top.txt is not there");
    assert_eq!(listed[1].path, folder("proj"));
    let (path, parent, listed) = dirs(&mut me, Some(&folder("proj"))).await.unwrap();
    assert_eq!((path, parent.as_deref(), names(&listed)), (folder("proj"), Some(folder("").as_str()), vec!["sub"]));
    for bad in [folder("../x"), folder("/etc"), folder("proj/nope"), "node:node-nobody:".to_string()] {
        assert!(dirs(&mut me, Some(&bad)).await.is_err(), "{bad}");
    }
    assert!(dirs(&mut me, None).await.is_ok(), "the hub's own folders are still there");

    // A conversation in `proj`: the model is told, and writes and reads there — on the node's disk.
    let (reply, _) = say(&mut me, "c1", "hello", Some(&folder("proj")), false).await;
    assert!(reply.contains("works in the folder 'proj'") && reply.contains("Test Node"), "{reply}");
    assert_eq!(workdir_of(&mut me, "c1").await, Some(folder("proj")));
    let (reply, asked) = say(&mut me, "c1", "WRITE", None, false).await;
    assert!(reply.contains("tool said") && !reply.contains("error"), "{reply}");
    assert_eq!((asked, std::fs::read_to_string(hub.shared().join("proj/out.txt")).unwrap()), (0, "from the hub".to_string()));
    assert!(!hub.shared().join("out.txt").exists() && !hub.dir.join("vault/out.txt").exists(), "not in the node's top folder, not in the hub's vault");
    let (reply, _) = say(&mut me, "c1", "READ", None, false).await;
    assert!(reply.contains("from the hub"), "{reply}");

    // It can't leave the folder with `..`.
    let (reply, _) = say(&mut me, "c1", "ESCAPE", None, false).await;
    assert!(reply.contains("inside the working folder"), "{reply}");
    assert!(!hub.shared().join("escaped.txt").exists() && !hub.shared().join("proj/escaped.txt").exists());

    // The shell always asks: a no runs nothing, a yes runs it in the folder.
    let (reply, asked) = say(&mut me, "c1", "RUN", None, false).await;
    assert_eq!(asked, 1);
    assert!(reply.contains("did not approve"), "{reply}");
    assert!(!hub.shared().join("proj/made.txt").exists());
    let (_, asked) = say(&mut me, "c1", "RUN", None, true).await;
    assert_eq!(asked, 1);
    assert_eq!(std::fs::read_to_string(hub.shared().join("proj/made.txt")).unwrap().trim(), "ran");

    // The folder is the conversation's for good, and every call is in the node's audit log.
    let (reply, _) = say(&mut me, "c1", "again", Some(&folder("other")), false).await;
    assert!(reply.contains("works in the folder 'proj'"), "{reply}");
    let audit = std::fs::read_to_string(hub.dir.join("node_audit.jsonl")).unwrap();
    assert!(audit.contains("\"op\":\"list_dirs\"") && audit.contains("\"op\":\"write_file\"") && audit.contains("\"op\":\"shell\""), "{audit}");
}

#[tokio::test]
async fn a_folder_on_a_node_that_is_gone_or_never_there_is_an_error_and_nothing_is_saved() {
    let hub = spin_up().await;
    let (mut me, node) = owner_with_node(&hub).await;
    let (reply, _) = say(&mut me, "c1", "hello", Some(&folder("nope")), false).await;
    assert!(reply.starts_with("error:"), "{reply}");
    let (reply, _) = say(&mut me, "c2", "hello", Some("node:node-nobody:proj"), false).await;
    assert!(reply.starts_with("error:") && reply.contains("no node"), "{reply}");
    me.send(&ClientMessage::ListConversations { request_id: 10 }).await.unwrap();
    match next(&mut me).await {
        ServerMessage::ConversationList { conversations, .. } => assert!(conversations.is_empty(), "{conversations:?}"),
        other => panic!("{other:?}"),
    }

    // A conversation that exists keeps working only while its node is there.
    let (reply, _) = say(&mut me, "c3", "hello", Some(&folder("proj")), false).await;
    assert!(reply.contains("works in the folder"), "{reply}");
    node.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (reply, _) = say(&mut me, "c3", "again", None, false).await;
    assert!(reply.starts_with("error:"), "{reply}");
}

#[tokio::test]
async fn a_member_only_browses_and_works_in_the_node_folders_the_owner_named_for_them() {
    let hub = spin_up().await;
    let (_owner, _node) = owner_with_node(&hub).await;
    let mut ana = ana(&hub).await;

    // Their start has the folder named for them, by the node's name; from it the way up is back to that list.
    let (path, parent, listed) = dirs(&mut ana, None).await.unwrap();
    assert_eq!((path.as_str(), parent), ("", None));
    assert_eq!(listed, vec![DirEntryDto { name: "Test Node · proj".into(), path: folder("proj") }], "and no folder of the hub's machine, which she was given none of");
    let (_, parent, inner) = dirs(&mut ana, Some(&folder("proj"))).await.unwrap();
    assert_eq!((parent.as_deref(), names(&inner)), (Some(""), vec!["sub"]));
    let (_, parent, _) = dirs(&mut ana, Some(&folder("proj/sub"))).await.unwrap();
    assert_eq!(parent, Some(folder("proj")));
    for outside in [folder(""), folder("other"), folder("proj/../other")] {
        assert!(dirs(&mut ana, Some(&outside)).await.is_err(), "{outside}");
    }

    // Working: in `proj` (and deeper) yes, elsewhere on the node no; files are hers to write, the shell is not given.
    let (reply, _) = say(&mut ana, "a1", "WRITE", Some(&folder("proj")), false).await;
    assert!(reply.contains("tool said") && !reply.contains("error"), "{reply}");
    assert_eq!(std::fs::read_to_string(hub.shared().join("proj/out.txt")).unwrap(), "from the hub");
    let (reply, _) = say(&mut ana, "a2", "hello", Some(&folder("proj/sub")), false).await;
    assert!(reply.contains("works in the folder 'proj/sub'"), "{reply}");
    for outside in [folder("other"), folder("")] {
        let (reply, _) = say(&mut ana, "a3", "hello", Some(&outside), false).await;
        assert!(reply.starts_with("error:") && reply.contains("not a folder on a node you can work in"), "{outside}: {reply}");
    }
    let (reply, asked) = say(&mut ana, "a1", "RUN", None, true).await;
    assert_eq!(asked, 0, "she is never asked, and has no shell to ask with");
    assert!(!hub.shared().join("proj/made.txt").exists(), "{reply}");

    // The owner takes the folder away: her conversation in it stops on its next turn.
    let mut config = load_config_from_path(&hub.config_path, false).unwrap();
    config.users[0].node_workdirs.clear();
    save_config(&hub.config_path, &config).unwrap();
    let (reply, _) = say(&mut ana, "a1", "again", None, false).await;
    assert!(reply.starts_with("error:") && reply.contains("not a folder on a node you can work in"), "{reply}");
    assert!(dirs(&mut ana, None).await.unwrap().2.is_empty());
}
