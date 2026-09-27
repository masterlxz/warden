//! P84, fatia 1, on a real hub: the owner (pairing key) and a member (username and password) on the
//! same `Server` at once. The member starts on a provisional password and can do nothing but change
//! it; then their agent writes to their own vault, never the owner's; conversations don't mix; they
//! get none of the tools or screens that reach what's the owner's; and removing them closes them out.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use warden_bootstrap::tasks::TaskStore;
use warden_bootstrap::{save_config, save_conversation, AgentConfig, Conversation, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::file_tools::WriteFileTool;
use warden_core::tool::shell::ShellTool;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};

const KEY: &str = "test-key";
const TEMP: &str = "provisional-1";

type Offered = Arc<Mutex<Vec<Vec<String>>>>;

/// "WRITE <path>" asks for `write_file` there; a tool result ends the turn.
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
        if let Some(path) = last.content.strip_prefix("WRITE ") {
            return Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall { id: "c1".into(), name: "write_file".into(), arguments: json!({ "path": path, "content": "written" }), thought_signature: None }],
                usage: None,
            }));
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

struct Hub {
    url: String,
    dir: PathBuf,
    offered: Offered,
}

async fn spin_up() -> Hub {
    let dir = std::env::temp_dir().join(format!("warden-server-people-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    let mut config = FileConfig {
        agents: vec![AgentConfig {
            id: "helper".into(),
            persona: "You help.".into(),
            provider_id: None,
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            owner: None,
            shared_with: Vec::new(),
        }],
        ..FileConfig::default()
    };
    warden_bootstrap::users::add_user(&mut config, "ana", "Ana", TEMP).unwrap();
    save_config(&config_path, &config).unwrap();

    // A scheduled task's conversation: the owner's, never listed to a member.
    let tasks = TaskStore::new(dir.join("tasks"));
    save_conversation(&tasks.conversations_dir(), &Conversation { id: "task-news".into(), title: "News".into(), messages: Vec::new(), created_at: 1, updated_at: 1, agent_id: None, provider_id: None }).unwrap();

    let offered: Offered = Arc::default();
    let vault = Arc::new(Vault::new(dir.join("vault")));
    let mut orchestrator = Orchestrator::new(Arc::new(Scripted { offered: offered.clone() }), vault.clone());
    orchestrator.register_tool(Arc::new(WriteFileTool::new(vault.clone())));
    orchestrator.register_tool(Arc::new(ShellTool::new(vault)));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(TestHost { path: config_path }))
        .with_users_dir(dir.join("users"))
        .with_tasks(tasks, false)
        .with_revocation_check_interval(Duration::from_millis(50))
        .with_node_audit(None);
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}"), dir, offered }
}

async fn member(hub: &Hub, password: &str, token: Option<String>) -> anyhow::Result<(ServerConnection, Option<String>, Option<warden_server_protocol::protocol::UserInfoDto>)> {
    ServerConnection::handshake_as_member(&hub.url, "ana-phone", "Ana's phone", "ana", password, token, warden_server_protocol::tls::default_client_config()).await
}

/// The next reply that isn't a stray notice.
async fn reply(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), conn.recv()).await.expect("no reply").unwrap().expect("connection closed") {
            ServerMessage::ConversationsChanged { .. } | ServerMessage::Pong { .. } => continue,
            msg => return msg,
        }
    }
}

async fn chat(conn: &mut ServerConnection, message: &str, conversation: &str) -> ServerMessage {
    conn.send(&ClientMessage::Chat { message: message.into(), conversation_id: Some(conversation.into()), attachments: Vec::new(), agent_id: Some("helper".into()) }).await.unwrap();
    reply(conn).await
}

async fn conversation_ids(conn: &mut ServerConnection) -> Vec<String> {
    conn.send(&ClientMessage::ListConversations { request_id: 9 }).await.unwrap();
    match reply(conn).await {
        ServerMessage::ConversationList { conversations, .. } => {
            let mut ids: Vec<String> = conversations.into_iter().map(|c| c.id).collect();
            ids.sort();
            ids
        }
        other => panic!("{other:?}"),
    }
}

fn last_offered(hub: &Hub) -> Vec<String> {
    hub.offered.lock().unwrap().last().cloned().unwrap_or_default()
}

#[tokio::test]
async fn the_owner_and_a_member_share_a_hub_without_sharing_anything_else() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();

    // Wrong password: turned away. Right one: paired as Ana, on a provisional password.
    assert!(member(&hub, "not-her-password", None).await.is_err());
    let (mut ana, token, user) = member(&hub, TEMP, None).await.unwrap();
    let user = user.expect("a member's device says who it is");
    assert_eq!((user.id.as_str(), user.must_change_password), ("ana", true));
    let token = token.expect("a token to come back with");

    // Until she picks her own password, nothing else goes through.
    match chat(&mut ana, "hello", "c1").await {
        ServerMessage::ChatError { message, .. } => assert!(message.contains("your own password"), "{message}"),
        other => panic!("{other:?}"),
    }
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: "wrong-one".into(), new_password: "anas-own-pass".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    ana.send(&ClientMessage::ChangePassword { request_id: 2, old_password: TEMP.into(), new_password: "anas-own-pass".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { request_id: 2 }));

    // Her agent writes to her vault, with none of the owner's tools.
    let answer = chat(&mut ana, "WRITE notes/ana.md", "ana-chat").await;
    assert!(matches!(answer, ServerMessage::ChatResponse { .. }), "{answer:?}");
    assert_eq!(std::fs::read_to_string(hub.dir.join("users/ana/vault/notes/ana.md")).unwrap(), "written");
    assert!(!hub.dir.join("vault/notes/ana.md").exists(), "never the owner's vault");
    assert!(last_offered(&hub).contains(&"write_file".to_string()));
    assert!(!last_offered(&hub).contains(&"shell".to_string()), "{:?}", last_offered(&hub));

    // The owner's agent still has everything, on the owner's vault.
    chat(&mut owner, "WRITE notes/mine.md", "owner-chat").await;
    assert!(hub.dir.join("vault/notes/mine.md").exists());
    assert!(last_offered(&hub).contains(&"shell".to_string()));

    // Conversations don't mix, and the scheduled task's is the owner's.
    assert_eq!(conversation_ids(&mut ana).await, ["ana-chat"]);
    assert_eq!(conversation_ids(&mut owner).await, ["owner-chat", "task-news"]);

    // No administration for her; settings show only the agents.
    ana.send(&ClientMessage::ListDevices { request_id: 3 }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::DeviceError { auth_rejected: true, .. }));
    ana.send(&ClientMessage::ListUsers { request_id: 4 }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    ana.send(&ClientMessage::RequestSettings { request_id: 5 }).await.unwrap();
    match reply(&mut ana).await {
        ServerMessage::Settings { settings, .. } => {
            assert_eq!(settings.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["helper"]);
            assert!(settings.tool_names.is_empty() && settings.providers.is_empty());
        }
        other => panic!("{other:?}"),
    }

    // Her token brings her back as herself, on her own password now.
    let (mut again, _, user) = member(&hub, "", Some(token.clone())).await.unwrap();
    assert!(!user.unwrap().must_change_password);
    assert_eq!(conversation_ids(&mut again).await, ["ana-chat"]);

    // The owner sees whose device it is, lists and removes her; her connections close.
    owner.send(&ClientMessage::ListDevices { request_id: 6 }).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::DeviceList { devices, .. } => {
            let phone = devices.iter().find(|d| d.device_id == "ana-phone").unwrap();
            assert_eq!(phone.user.as_deref(), Some("ana"));
        }
        other => panic!("{other:?}"),
    }
    owner.send(&ClientMessage::RemoveUser { request_id: 7, pairing_key: "wrong".into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }));
    owner.send(&ClientMessage::RemoveUser { request_id: 8, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserList { users, .. } if users.is_empty()));
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match again.recv().await {
                Ok(Some(ServerMessage::AuthError { .. })) | Ok(None) | Err(_) => return,
                Ok(Some(_)) => continue,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "her open connection was closed");
    assert!(member(&hub, "", Some(token)).await.is_err(), "and her token no longer works");
    assert!(hub.dir.join("users/ana/vault/notes/ana.md").exists(), "her things stay on disk");
}

#[tokio::test]
async fn the_owner_creates_and_resets_members_with_a_password_shown_once() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    owner.send(&ClientMessage::SaveUser { request_id: 1, pairing_key: KEY.into(), id: "bruno".into(), name: "Bruno".into(), is_new: true }).await.unwrap();
    let temp = match reply(&mut owner).await {
        ServerMessage::UserList { users, temp_password, .. } => {
            assert_eq!(users.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["ana", "bruno"]);
            temp_password.expect("shown once")
        }
        other => panic!("{other:?}"),
    };
    let config = std::fs::read_to_string(hub.dir.join("config.toml")).unwrap();
    assert!(!config.contains(&temp), "only the hash is written");
    let (_bruno, _, user) =
        ServerConnection::handshake_as_member(&hub.url, "bruno-pc", "Bruno's PC", "bruno", &temp, None, warden_server_protocol::tls::default_client_config()).await.unwrap();
    assert!(user.unwrap().must_change_password);

    owner.send(&ClientMessage::ResetPassword { request_id: 2, pairing_key: KEY.into(), id: "bruno".into() }).await.unwrap();
    let reset = match reply(&mut owner).await {
        ServerMessage::UserList { temp_password, .. } => temp_password.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_ne!(reset, temp);
    // The owner has no password to change.
    owner.send(&ClientMessage::ChangePassword { request_id: 3, old_password: "x".into(), new_password: "y-long-enough".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: false, .. }));
}
