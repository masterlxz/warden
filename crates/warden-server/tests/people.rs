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
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall, Usage};
use warden_core::spend::{Limit, MemoryStore, PriceTable, Scope, SpendGuard};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::file_tools::WriteFileTool;
use warden_core::tool::shell::ShellTool;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};

const KEY: &str = "test-key";
const TEMP: &str = "provisional-1";

type Offered = Arc<Mutex<Vec<Vec<String>>>>;

/// "WRITE <path>" asks for `write_file` there; a tool result ends the turn; "SPEND" costs 100 tokens.
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
        let usage = last.content.contains("SPEND").then_some(Usage { prompt_tokens: 100, completion_tokens: 0, total_tokens: 100 });
        Ok(response_stream(Response { content: "plain".into(), tool_calls: Vec::new(), usage }))
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
            shared_with: vec!["ana".into()],
        }],
        ..FileConfig::default()
    };
    // Fatia 2: one agent the owner keeps to themselves, one shared with everyone but narrowed.
    let helper = config.agents[0].clone();
    config.agents.push(AgentConfig { id: "private".into(), shared_with: Vec::new(), ..helper.clone() });
    config.agents.push(AgentConfig { id: "family".into(), shared_with: vec!["*".into()], allowed_tools: Some(vec!["write_file".into(), "shell".into()]), ..helper });
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
    // Fatia 2: Ana may spend 150 tokens a day, on every channel; the owner has no limit.
    let limits = vec![Limit::new("ana-day", Scope::Person("ana".into()), 24).with_max_tokens(150)];
    let orchestrator = orchestrator.with_spend_guard(Arc::new(SpendGuard::new(Arc::new(MemoryStore::default()), limits, PriceTable::new(Vec::new()))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(TestHost { path: config_path }))
        .with_users_dir(dir.join("users"))
        .with_tasks(tasks, false)
        .with_revocation_check_interval(Duration::from_millis(50))
        .with_api(dir.join("api_keys.json"))
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
            // The agents shared with her (not the owner's private one), and her own tools.
            assert_eq!(settings.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["helper", "family"]);
            assert_eq!(settings.tool_names, ["write_file"]);
            assert!(settings.providers.is_empty());
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

async fn chat_as(conn: &mut ServerConnection, message: &str, conversation: &str, agent: &str) -> ServerMessage {
    conn.send(&ClientMessage::Chat { message: message.into(), conversation_id: Some(conversation.into()), attachments: Vec::new(), agent_id: Some(agent.into()) }).await.unwrap();
    reply(conn).await
}

/// Ana, past her provisional password.
async fn ana_ready(hub: &Hub) -> ServerConnection {
    let (mut ana, _, _) = member(hub, TEMP, None).await.unwrap();
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: TEMP.into(), new_password: "anas-own-pass".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { .. }));
    ana
}

fn agent_ids(message: &ServerMessage) -> Vec<(String, Option<String>)> {
    match message {
        ServerMessage::Settings { settings, .. } => settings.agents.iter().map(|a| (a.id.clone(), a.owner.clone())).collect(),
        other => panic!("{other:?}"),
    }
}

/// Fatia 2: the owner decides which agents and tools a member has, a member's own agents stay
/// theirs, and a person's spending limit stops them without touching anyone else.
#[tokio::test]
async fn what_a_member_may_use_is_what_the_owner_shares_and_allows() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let mut ana = ana_ready(&hub).await;

    // She sees the shared agents only, and can't talk to the owner's private one.
    ana.send(&ClientMessage::RequestSettings { request_id: 2 }).await.unwrap();
    let mut seen = agent_ids(&reply(&mut ana).await);
    seen.sort();
    assert_eq!(seen, [("family".to_string(), None), ("helper".to_string(), None)]);
    match chat_as(&mut ana, "hi", "a1", "private").await {
        ServerMessage::ChatError { message, .. } => assert!(message.contains("not found"), "{message}"),
        other => panic!("{other:?}"),
    }

    // A shared agent: her tools ∩ the agent's. No shell until the owner allows it.
    assert!(matches!(chat_as(&mut ana, "hi", "a1", "family").await, ServerMessage::ChatResponse { .. }));
    assert_eq!(last_offered(&hub), ["write_file"]);
    owner.send(&ClientMessage::SetUserTools { request_id: 3, pairing_key: "wrong".into(), id: "ana".into(), tools: Some(vec!["shell".into()]) }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }));
    let tools = vec!["write_file".to_string(), "shell".to_string(), "delegate_to_agent".to_string()];
    owner.send(&ClientMessage::SetUserTools { request_id: 4, pairing_key: KEY.into(), id: "ana".into(), tools: Some(tools) }).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::UserList { users, .. } => assert_eq!(users[0].tools.as_deref(), Some(&["shell".to_string(), "write_file".to_string()][..]), "never delegate_to_agent"),
        other => panic!("{other:?}"),
    }
    chat_as(&mut ana, "hi", "a1", "family").await;
    let mut offered = last_offered(&hub);
    offered.sort();
    assert_eq!(offered, ["shell", "write_file"]);

    // Her own agent: hers alone, powerless, within her tools.
    let own = warden_server_protocol::protocol::AgentSettingsDto {
        original_id: None,
        id: "cook".into(),
        persona: "You cook.".into(),
        provider_id: String::new(),
        can_delegate_to_agents: true,
        can_manage_agents: true,
        can_message_agents: false,
        can_manage_tasks: false,
        allowed_tools: Some(vec!["write_file".into(), "read_file".into()]),
        shared_with: vec!["*".into()],
        owner: None,
    };
    ana.send(&ClientMessage::SaveOwnAgent { request_id: 5, original_id: None, agent: own }).await.unwrap();
    assert!(agent_ids(&reply(&mut ana).await).contains(&("cook".to_string(), Some("ana".to_string()))));
    assert!(matches!(chat_as(&mut ana, "hi", "a2", "cook").await, ServerMessage::ChatResponse { .. }));
    assert_eq!(last_offered(&hub), ["write_file"], "read_file isn't among her tools any more");
    let config = warden_bootstrap::load_config_from_path(&hub.dir.join("config.toml"), true).unwrap();
    let cook = config.agents.iter().find(|a| a.id == "cook").unwrap();
    assert!(!cook.can_manage_agents && !cook.can_delegate_to_agents && cook.shared_with.is_empty());

    // The owner neither sees nor uses it, and can't delete it as theirs.
    owner.send(&ClientMessage::RequestSettings { request_id: 6 }).await.unwrap();
    assert!(!agent_ids(&reply(&mut owner).await).iter().any(|(id, _)| id == "cook"));
    assert!(matches!(chat_as(&mut owner, "hi", "o1", "cook").await, ServerMessage::ChatError { .. }));
    owner.send(&ClientMessage::DeleteOwnAgent { request_id: 7, id: "cook".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::SettingsError { .. }));
    ana.send(&ClientMessage::DeleteOwnAgent { request_id: 8, id: "helper".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::SettingsError { .. }), "not hers to delete");

    // Her limit: 100 tokens in, a turn that would pass 150 is stopped; the owner goes on.
    assert!(matches!(chat_as(&mut ana, "SPEND", "a3", "family").await, ServerMessage::ChatResponse { .. }));
    assert!(matches!(chat_as(&mut ana, "SPEND", "a3", "family").await, ServerMessage::ChatResponse { .. }));
    match chat_as(&mut ana, "SPEND", "a3", "family").await {
        ServerMessage::ChatError { spend_limit_id, .. } => assert_eq!(spend_limit_id.as_deref(), Some("ana-day")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(chat_as(&mut owner, "SPEND", "o2", "helper").await, ServerMessage::ChatResponse { .. }));
}

/// A plain HTTP/1.1 request to the hub's Warden API; the status and the body.
async fn http(url: &str, method: &str, path: &str, key: &str, body: Option<&str>) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = url.trim_start_matches("ws://");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {key}\r\n");
    if let Some(body) = body {
        request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body.unwrap_or_default());
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    (head.split_whitespace().nth(1).unwrap().parse().unwrap(), body.to_string())
}

/// Fatia 2: a member's own Warden API keys — confirmed with her password, listed only to her,
/// speaking as her (her vault, the agents she sees), and gone with her.
#[tokio::test]
async fn a_member_keeps_api_keys_of_her_own() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let mut ana = ana_ready(&hub).await;

    ana.send(&ClientMessage::CreateApiKey { request_id: 2, pairing_key: KEY.into(), name: "script".into(), agent_id: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::ApiKeyError { auth_rejected: true, .. }), "her password, not the pairing key");
    ana.send(&ClientMessage::CreateApiKey { request_id: 3, pairing_key: "anas-own-pass".into(), name: "anas-bot".into(), agent_id: Some("private".into()) }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::ApiKeyError { auth_rejected: false, .. }), "an agent she doesn't see");
    ana.send(&ClientMessage::CreateApiKey { request_id: 4, pairing_key: "anas-own-pass".into(), name: "anas-script".into(), agent_id: None }).await.unwrap();
    let anas_key = match reply(&mut ana).await {
        ServerMessage::ApiKeyCreated { key, keys, .. } => {
            assert_eq!(keys.iter().map(|k| (k.name.as_str(), k.user.as_deref())).collect::<Vec<_>>(), [("anas-script", Some("ana"))]);
            key
        }
        other => panic!("{other:?}"),
    };
    owner.send(&ClientMessage::CreateApiKey { request_id: 5, pairing_key: KEY.into(), name: "owners".into(), agent_id: None }).await.unwrap();
    let ServerMessage::ApiKeyCreated { keys, .. } = reply(&mut owner).await else { panic!() };
    assert_eq!(keys.len(), 2, "the owner sees every key, with whose it is");
    let owners_id = keys.iter().find(|k| k.name == "owners").unwrap().id.clone();
    ana.send(&ClientMessage::ListApiKeys { request_id: 6 }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::ApiKeyList { keys, .. } if keys.len() == 1));
    ana.send(&ClientMessage::RevokeApiKey { request_id: 7, pairing_key: "anas-own-pass".into(), id: owners_id }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::ApiKeyError { .. }), "not hers to revoke");

    // Her key speaks as her: the agents she sees, her vault.
    let (status, body) = http(&hub.url, "GET", "/v1/models", &anas_key, None).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("warden/family") && body.contains("warden/helper") && !body.contains("warden/private"), "{body}");
    let request = r#"{"model":"warden","messages":[{"role":"user","content":"WRITE notes/from-api.md"}]}"#;
    let (status, body) = http(&hub.url, "POST", "/v1/chat/completions", &anas_key, Some(request)).await;
    assert_eq!(status, 200, "{body}");
    assert!(hub.dir.join("users/ana/vault/notes/from-api.md").exists());
    assert!(!hub.dir.join("vault/notes/from-api.md").exists());
    let (status, _) = http(&hub.url, "POST", "/v1/chat/completions", &anas_key, Some(r#"{"model":"warden/private","messages":[{"role":"user","content":"hi"}]}"#)).await;
    assert_eq!(status, 404, "the owner's private agent isn't a model for her");

    // Removing her takes her keys along.
    owner.send(&ClientMessage::RemoveUser { request_id: 8, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserList { .. }));
    let (status, _) = http(&hub.url, "GET", "/v1/models", &anas_key, None).await;
    assert_eq!(status, 401);
}
