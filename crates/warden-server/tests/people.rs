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
use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
use warden_core::tool::shell::ShellTool;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};

const KEY: &str = "test-key";
const TEMP: &str = "provisional-1";

type Offered = Arc<Mutex<Vec<Vec<String>>>>;

/// "WRITE <path>" asks for `write_file` there, "READ <path>" for `read_file`; a tool result ends the
/// turn; "SPEND" costs 100 tokens.
struct Scripted {
    offered: Offered,
}

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        self.offered.lock().unwrap().push(tools.iter().map(|t| t.name.clone()).collect());
        let last = messages.last().unwrap();
        // P104: what the hub asks after a turn, to see whether the assistant learned something.
        if messages.first().is_some_and(|m| m.content.contains("whether the latest turn")) {
            let signal = if last.content.contains("<person>Nao, sempre separe") { "correction" } else { "none" };
            return Ok(response_stream(Response { content: format!(r#"{{"signal":"{signal}"}}"#), tool_calls: Vec::new(), usage: None }));
        }
        if messages.first().is_some_and(|m| m.content.contains("reusable skill for an AI assistant")) {
            // The skill already exists (its text is in the request): the lesson belongs in it.
            if last.content.contains("<skill name=\"release-notes-style\">") {
                let revise = r#"{"revise":{"name":"release-notes-style","description":"How to lay out release notes.","body":"Group them under Added, Fixed and Removed, with no emoji.","rationale":"The person corrected the layout."}}"#;
                return Ok(response_stream(Response { content: revise.to_string(), tool_calls: Vec::new(), usage: None }));
            }
            let skill = r#"{"skill":{"name":"release-notes-style","description":"How to lay out release notes.","body":"Group them under Added, Fixed and Removed, with no emoji.","rationale":"The person corrected the layout."}}"#;
            return Ok(response_stream(Response { content: skill.to_string(), tool_calls: Vec::new(), usage: None }));
        }
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
        if let Some(words) = last.content.strip_prefix("SEARCH ") {
            return Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall { id: "c1".into(), name: "search_history".into(), arguments: json!({ "query": words }), thought_signature: None }],
                usage: None,
            }));
        }
        if let Some(path) = last.content.strip_prefix("READ ") {
            return Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall { id: "c1".into(), name: "read_file".into(), arguments: json!({ "path": path }), thought_signature: None }],
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
    // These tests don't download or run the embedding model: `search_history` falls back to words.
    std::env::set_var(warden_core::memory::embed::OFF_SWITCH, "1");
    // A counter too: two tests starting in the same nanosecond must not share a hub's files.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("warden-server-people-{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
            autonomy: warden_bootstrap::default_autonomy(),
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
    save_conversation(&tasks.conversations_dir(), &Conversation { id: "task-news".into(), title: "News".into(), messages: Vec::new(), created_at: 1, updated_at: 1, agent_id: None, provider_id: None, project_id: None, engine_session_id: None, workdir: None }).unwrap();

    let offered: Offered = Arc::default();
    let vault = Arc::new(Vault::new(dir.join("vault")));
    let mut orchestrator = Orchestrator::new(Arc::new(Scripted { offered: offered.clone() }), vault.clone());
    orchestrator.register_tool(Arc::new(WriteFileTool::new(vault.clone())));
    orchestrator.register_tool(Arc::new(ReadFileTool::new(vault.clone())));
    orchestrator.register_tool(Arc::new(ShellTool::new(vault)));
    // P104: built without a folder, like the hub's own orchestrator; the hub points it at whoever is speaking.
    orchestrator.register_tool(Arc::new(warden_bootstrap::history::SearchHistoryTool::new(None)));
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
    conn.send(&ClientMessage::Chat { message: message.into(), conversation_id: Some(conversation.into()), attachments: Vec::new(), agent_id: Some("helper".into()), project_id: None, workdir: None }).await.unwrap();
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
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: "wrong-one".into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    ana.send(&ClientMessage::ChangePassword { request_id: 2, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { request_id: 2, .. }));

    // Her agent writes to her vault, with none of the owner's tools.
    let answer = chat(&mut ana, "WRITE notes/ana.md", "ana-chat").await;
    assert!(matches!(answer, ServerMessage::ChatResponse { .. }), "{answer:?}");
    assert_eq!(ana_vault(&hub, "anas-own-pass").read("notes/ana.md").unwrap(), "written");
    assert!(!hub.dir.join("users/ana/vault/notes/ana.md").exists(), "what she writes is encrypted on disk, name and all");
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
            assert_eq!(settings.tool_names, ["write_file", "read_file", "search_history"]);
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
    // Her data stays on disk, and her entry — the wrapped key — is kept as a removed member's, so the
    // same password still opens it (and `users restore` can bring her back).
    let config = warden_bootstrap::load_config_from_path(&hub.dir.join("config.toml"), false).unwrap();
    assert!(config.users.is_empty());
    let key = warden_bootstrap::users::open_key(&config.removed_users[0], "anas-own-pass").unwrap().expect("her key is kept");
    let her_vault = Vault::new_encrypted(hub.dir.join("users/ana/vault"), Arc::new(warden_core::memory::VaultCipher::new(&key)));
    assert!(her_vault.read("notes/ana.md").is_ok(), "her things stay on disk");
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
    owner.send(&ClientMessage::ChangePassword { request_id: 3, old_password: "x".into(), new_password: "y-long-enough".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: false, .. }));
}

async fn chat_as(conn: &mut ServerConnection, message: &str, conversation: &str, agent: &str) -> ServerMessage {
    conn.send(&ClientMessage::Chat { message: message.into(), conversation_id: Some(conversation.into()), attachments: Vec::new(), agent_id: Some(agent.into()), project_id: None, workdir: None }).await.unwrap();
    reply(conn).await
}

/// Ana, past her provisional password.
async fn ana_ready(hub: &Hub) -> ServerConnection {
    let (mut ana, _, _) = member(hub, TEMP, None).await.unwrap();
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { .. }));
    ana
}

/// Ana's vault as the hub opens it: her data is encrypted on disk, so the test needs her password
/// (P84 fatia 4) to read what her agent wrote.
fn ana_vault(hub: &Hub, password: &str) -> Vault {
    let config = warden_bootstrap::load_config_from_path(&hub.dir.join("config.toml"), false).unwrap();
    let user = config.users.iter().find(|u| u.id == "ana").unwrap();
    let key = warden_bootstrap::users::open_key(user, password).unwrap().expect("her data has a key");
    Vault::new_encrypted(hub.dir.join("users/ana/vault"), Arc::new(warden_core::memory::VaultCipher::new(&key)))
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
        autonomy: 4,
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
        // She can't allow more, so she's told to ask the owner, with no limit for her to extend.
        ServerMessage::ChatError { spend_limit_id, message, .. } => {
            assert_eq!(spend_limit_id, None);
            assert!(message.contains("ana-day") && message.contains("Ask whoever runs this Warden to allow more"), "{message}");
            assert!(!message.contains("desktop") && !message.contains("Usage tab"), "{message}");
        }
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
    assert!(ana_vault(&hub, "anas-own-pass").read("notes/from-api.md").is_ok());
    assert!(!hub.dir.join("vault/notes/from-api.md").exists());
    let (status, _) = http(&hub.url, "POST", "/v1/chat/completions", &anas_key, Some(r#"{"model":"warden/private","messages":[{"role":"user","content":"hi"}]}"#)).await;
    assert_eq!(status, 404, "the owner's private agent isn't a model for her");

    // Fatia 3: a space shared with her reaches her key too.
    std::fs::create_dir_all(hub.dir.join("vault/casa")).unwrap();
    std::fs::write(hub.dir.join("vault/casa/lista.md"), "arroz").unwrap();
    owner.send(&ClientMessage::SaveSpace { request_id: 9, pairing_key: KEY.into(), original_id: None, space: space("casa", "casa", &["ana"], &[]) }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::SpaceList { .. }));
    let request = r#"{"model":"warden","messages":[{"role":"user","content":"READ compartilhado/casa/lista.md"}]}"#;
    let (status, body) = http(&hub.url, "POST", "/v1/chat/completions", &anas_key, Some(request)).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("arroz"), "{body}");

    // Removing her takes her keys along.
    owner.send(&ClientMessage::RemoveUser { request_id: 10, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserList { .. }));
    let (status, _) = http(&hub.url, "GET", "/v1/models", &anas_key, None).await;
    assert_eq!(status, 401);
}

fn tool_said(message: ServerMessage) -> String {
    match message {
        ServerMessage::ChatResponse { content, .. } => content,
        other => panic!("{other:?}"),
    }
}

fn space(id: &str, folder: &str, readers: &[&str], writers: &[&str]) -> warden_server_protocol::protocol::SpaceDto {
    warden_server_protocol::protocol::SpaceDto {
        id: id.into(),
        folder: folder.into(),
        readers: readers.iter().map(|s| s.to_string()).collect(),
        writers: writers.iter().map(|s| s.to_string()).collect(),
    }
}

/// Fatia 3: the owner shares a folder of their vault; Ana's agent reads it at `compartilhado/casa/`,
/// writes there only once she's a writer, and loses it when the owner stops sharing it — never
/// seeing the rest of the owner's vault.
#[tokio::test]
async fn a_member_sees_only_the_folders_the_owner_shares_with_her() {
    let hub = spin_up().await;
    std::fs::create_dir_all(hub.dir.join("vault/casa")).unwrap();
    std::fs::write(hub.dir.join("vault/casa/lista.md"), "arroz, feijão").unwrap();
    std::fs::write(hub.dir.join("vault/segredo.md"), "the owner's").unwrap();
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let mut ana = ana_ready(&hub).await;

    // Nothing shared yet: the folder isn't there for her.
    let said = tool_said(chat(&mut ana, "READ compartilhado/casa/lista.md", "s1").await);
    assert!(said.contains("no shared space 'casa'"), "{said}");

    // Only the owner shares, with the pairing key.
    ana.send(&ClientMessage::SaveSpace { request_id: 2, pairing_key: KEY.into(), original_id: None, space: space("casa", "casa", &["ana"], &[]) }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    owner.send(&ClientMessage::SaveSpace { request_id: 3, pairing_key: "wrong".into(), original_id: None, space: space("casa", "casa", &["ana"], &[]) }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }));
    owner.send(&ClientMessage::SaveSpace { request_id: 4, pairing_key: KEY.into(), original_id: None, space: space("casa", "../vault", &["ana"], &[]) }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: false, .. }));
    owner.send(&ClientMessage::SaveSpace { request_id: 5, pairing_key: KEY.into(), original_id: None, space: space("casa", "casa", &["ana"], &[]) }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::SpaceList { spaces, .. } if spaces == [space("casa", "casa", &["ana"], &[])]));

    // She sees it as hers to read, where it shows up in her vault.
    ana.send(&ClientMessage::ListSpaces { request_id: 6 }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::SpaceList { spaces, .. } if spaces == [space("casa", "compartilhado/casa", &["ana"], &[])]));
    ana.send(&ClientMessage::ListVaultFiles { request_id: 7 }).await.unwrap();
    match reply(&mut ana).await {
        ServerMessage::VaultFileList { files, .. } => assert_eq!(files, ["compartilhado/casa/lista.md"], "and nothing else of the owner's"),
        other => panic!("{other:?}"),
    }
    let said = tool_said(chat(&mut ana, "READ compartilhado/casa/lista.md", "s1").await);
    assert!(said.contains("arroz, feijão"), "{said}");
    let said = tool_said(chat(&mut ana, "READ ../../../vault/segredo.md", "s1").await);
    assert!(!said.contains("the owner's"), "{said}");

    // Reading only: her agent's write is refused.
    let said = tool_said(chat(&mut ana, "WRITE compartilhado/casa/nova.md", "s1").await);
    assert!(said.contains("read-only"), "{said}");
    assert!(!hub.dir.join("vault/casa/nova.md").exists());

    // A writer now: the note lands in the owner's folder.
    owner.send(&ClientMessage::SaveSpace { request_id: 8, pairing_key: KEY.into(), original_id: Some("casa".into()), space: space("casa", "casa", &[], &["ana"]) }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::SpaceList { .. }));
    tool_said(chat(&mut ana, "WRITE compartilhado/casa/nova.md", "s1").await);
    assert_eq!(std::fs::read_to_string(hub.dir.join("vault/casa/nova.md")).unwrap(), "written");

    // No longer shared: gone for her, while the folder stays the owner's.
    owner.send(&ClientMessage::DeleteSpace { request_id: 9, pairing_key: KEY.into(), id: "casa".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::SpaceList { spaces, .. } if spaces.is_empty()));
    let said = tool_said(chat(&mut ana, "READ compartilhado/casa/lista.md", "s1").await);
    assert!(said.contains("no shared space 'casa'"), "{said}");
    assert!(hub.dir.join("vault/casa/lista.md").exists() && hub.dir.join("vault/casa/nova.md").exists());
}

// ---- fatia 4: a member's data is encrypted on disk ----------------------------------------------

/// Every folder and file name under `dir`, and every file's bytes, as one string.
fn everything_on_disk(dir: &std::path::Path) -> String {
    let mut seen = String::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        seen.push_str(&path.file_name().unwrap().to_string_lossy());
        if path.is_dir() {
            seen.push_str(&everything_on_disk(&path));
        } else {
            seen.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap()));
        }
    }
    seen
}

/// What the hub's memory forgets when it restarts: Ana's key.
fn forget_anas_key(hub: &Hub) {
    warden_bootstrap::member_crypto::lock(&[&hub.dir.join("users/ana"), &hub.dir.join("conversations/users/ana")]);
}

/// Ana on a fresh hub, past her provisional password: her token, her recovery code and her connection.
async fn ana_with_a_code(hub: &Hub) -> (ServerConnection, String, String) {
    let (mut ana, token, _) = member(hub, TEMP, None).await.unwrap();
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    match reply(&mut ana).await {
        ServerMessage::PasswordChanged { recovery_code: Some(code), .. } => (ana, token.unwrap(), code),
        other => panic!("her first password change hands her a recovery code: {other:?}"),
    }
}

#[tokio::test]
async fn a_members_data_is_encrypted_on_disk_and_locked_when_the_hub_no_longer_holds_her_key() {
    let hub = spin_up().await;
    let (mut ana, token, code) = ana_with_a_code(&hub).await;
    assert_eq!(code.len(), 39, "{code}");

    // Her agent writes and talks; nothing readable reaches the disk, not even the file's name.
    tool_said(chat(&mut ana, "WRITE notes/segredo-da-ana.md", "conversa").await);
    let disk = format!("{}{}", everything_on_disk(&hub.dir.join("users/ana")), everything_on_disk(&hub.dir.join("conversations/users/ana")));
    for secret in ["segredo-da-ana", "written", "WRITE", "notes"] {
        assert!(!disk.contains(secret), "'{secret}' is readable on disk");
    }
    assert_eq!(ana_vault(&hub, "anas-own-pass").read("notes/segredo-da-ana.md").unwrap(), "written");
    assert!(warden_bootstrap::users::open_key(&warden_bootstrap::load_config_from_path(&hub.dir.join("config.toml"), false).unwrap().users[0], TEMP).is_err(), "the owner's provisional password never opens it");

    // The hub restarts: her token gets her in, but her data stays shut.
    forget_anas_key(&hub);
    let (mut back, _, user) = member(&hub, "", Some(token.clone())).await.unwrap();
    let user = user.unwrap();
    assert!(user.encrypted && user.locked, "{user:?}");
    match chat(&mut back, "hello", "conversa").await {
        ServerMessage::ChatError { message, .. } => assert!(message.contains("locked"), "{message}"),
        other => panic!("{other:?}"),
    }
    back.send(&ClientMessage::ListConversations { request_id: 1 }).await.unwrap();
    assert!(matches!(reply(&mut back).await, ServerMessage::ConversationError { message, .. } if message.contains("locked")));
    back.send(&ClientMessage::ListVaultFiles { request_id: 2 }).await.unwrap();
    assert!(matches!(reply(&mut back).await, ServerMessage::VaultError { message, .. } if message.contains("locked")));
    assert!(!everything_on_disk(&hub.dir.join("users/ana")).contains("hello") && !hub.dir.join("users/ana/vault/notes").exists(), "nothing plain is written while it's shut");

    // Signing in with the password opens it again.
    let (mut open, _, user) = member(&hub, "anas-own-pass", Some(token)).await.unwrap();
    assert!(!user.unwrap().locked);
    assert_eq!(conversation_ids(&mut open).await, ["conversa"]);
    let said = tool_said(chat(&mut open, "READ notes/segredo-da-ana.md", "conversa").await);
    assert!(said.contains("written"), "{said}");
}

#[tokio::test]
async fn after_the_owner_resets_her_password_only_her_recovery_code_brings_the_data_back() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (mut ana, token, code) = ana_with_a_code(&hub).await;
    tool_said(chat(&mut ana, "WRITE notes/segredo.md", "conversa").await);
    forget_anas_key(&hub);

    owner.send(&ClientMessage::ResetPassword { request_id: 1, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    let temp = match reply(&mut owner).await {
        ServerMessage::UserList { temp_password, users, .. } => {
            assert!(users[0].needs_recovery, "the owner sees that she needs her code, not what she has");
            temp_password.unwrap()
        }
        other => panic!("{other:?}"),
    };
    let config = warden_bootstrap::load_config_from_path(&hub.dir.join("config.toml"), false).unwrap();
    assert!(warden_bootstrap::users::open_key(&config.users[0], &temp).unwrap().is_none(), "the temporary password opens nothing");

    // She signs in with it: still locked, and choosing a new password needs the code.
    let (mut ana, _, user) = member(&hub, &temp, Some(token)).await.unwrap();
    let user = user.unwrap();
    assert!(user.must_change_password && user.needs_recovery && user.locked, "{user:?}");
    let change = |request_id, recovery_code: Option<String>| ClientMessage::ChangePassword { request_id, old_password: temp.clone(), new_password: "anas-new-pass".into(), recovery_code };
    ana.send(&change(1, None)).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { message, .. } if message.contains("recovery code")));
    ana.send(&change(2, Some(warden_bootstrap::member_crypto::generate_recovery_code()))).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { .. }), "a wrong code");
    ana.send(&change(3, Some(code))).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { request_id: 3, recovery_code: None }), "she keeps the code she has");

    // Everything is back, on the new password.
    let said = tool_said(chat(&mut ana, "READ notes/segredo.md", "conversa").await);
    assert!(said.contains("written"), "{said}");
    assert_eq!(conversation_ids(&mut ana).await, ["conversa"]);
    assert_eq!(ana_vault(&hub, "anas-new-pass").read("notes/segredo.md").unwrap(), "written");
}

#[tokio::test]
async fn a_member_from_before_gets_her_data_encrypted_when_she_signs_in() {
    let hub = spin_up().await;
    // Ana as she was before fatia 4: her own password, plain files and a plain conversation.
    let config_path = hub.dir.join("config.toml");
    let mut config = warden_bootstrap::load_config_from_path(&config_path, false).unwrap();
    config.users[0].must_change_password = false;
    config.users[0].password_hash = warden_bootstrap::users::hash_password("anas-own-pass").unwrap();
    save_config(&config_path, &config).unwrap();
    std::fs::create_dir_all(hub.dir.join("users/ana/vault/notes")).unwrap();
    std::fs::write(hub.dir.join("users/ana/vault/notes/velha.md"), "nota antiga da ana").unwrap();
    save_conversation(&hub.dir.join("conversations/users/ana"), &Conversation { id: "velha".into(), title: "Conversa antiga".into(), messages: Vec::new(), created_at: 1, updated_at: 1, agent_id: None, provider_id: None, project_id: None, engine_session_id: None, workdir: None }).unwrap();

    let (mut ana, _, user) = member(&hub, "anas-own-pass", None).await.unwrap();
    assert!(user.unwrap().encrypted, "the sign-in turned it on");
    let code = match reply(&mut ana).await {
        ServerMessage::RecoveryCode { request_id: 0, code } => code,
        other => panic!("she's handed a recovery code right after signing in: {other:?}"),
    };
    assert_eq!(code.len(), 39);

    let disk = format!("{}{}", everything_on_disk(&hub.dir.join("users/ana")), everything_on_disk(&hub.dir.join("conversations/users/ana")));
    assert!(!disk.contains("nota antiga") && !disk.contains("Conversa antiga") && !disk.contains("velha.md"), "her old files were encrypted in place");
    let said = tool_said(chat(&mut ana, "READ notes/velha.md", "nova").await);
    assert!(said.contains("nota antiga da ana"), "{said}");
    assert_eq!(conversation_ids(&mut ana).await, ["nova", "velha"]);

    // Signing in again keeps the same key, and hands out no second code.
    let (mut again, _, _) = member(&hub, "anas-own-pass", None).await.unwrap();
    assert_eq!(conversation_ids(&mut again).await, ["nova", "velha"], "no stray message before the reply");
}

#[tokio::test]
async fn a_member_asks_for_a_new_recovery_code_with_her_password() {
    let hub = spin_up().await;
    let (mut ana, _, first) = ana_with_a_code(&hub).await;
    ana.send(&ClientMessage::RegenerateRecoveryCode { request_id: 5, password: "not-her-password".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    ana.send(&ClientMessage::RegenerateRecoveryCode { request_id: 6, password: "anas-own-pass".into() }).await.unwrap();
    let second = match reply(&mut ana).await {
        ServerMessage::RecoveryCode { request_id: 6, code } => code,
        other => panic!("{other:?}"),
    };
    assert_ne!(first, second);

    let config = warden_bootstrap::load_config_from_path(&hub.dir.join("config.toml"), false).unwrap();
    let wraps = config.users[0].key.clone().unwrap();
    assert!(warden_bootstrap::member_crypto::unwrap_with_code(&wraps.by_recovery, &second).is_ok());
    assert!(warden_bootstrap::member_crypto::unwrap_with_code(&wraps.by_recovery, &first).is_err(), "the old code stopped working");

    // The owner has no recovery code of this kind.
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    owner.send(&ClientMessage::RegenerateRecoveryCode { request_id: 7, password: "x".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { .. }));
}

// ---- fatia 4, parte B: recovery policies -------------------------------------------------------

/// The owner sets the workspace's recovery policy; the private half of their recovery key comes back
/// only when one was made.
async fn set_policy(owner: &mut ServerConnection, policy: &str) -> Option<String> {
    owner.send(&ClientMessage::SetRecoveryPolicy { request_id: 50, pairing_key: KEY.into(), policy: policy.into(), new_key: false }).await.unwrap();
    match reply(owner).await {
        ServerMessage::RecoveryPolicy { policy: set, secret, .. } => {
            assert_eq!(set, policy);
            secret
        }
        other => panic!("{other:?}"),
    }
}

/// The owner tries to recover `id` with `key` (and the person's `code`).
async fn recover(owner: &mut ServerConnection, id: &str, key: &str, code: Option<&str>) -> ServerMessage {
    owner.send(&ClientMessage::RecoverMember { request_id: 60, pairing_key: KEY.into(), id: id.into(), recovery_key: key.into(), code: code.map(String::from) }).await.unwrap();
    reply(owner).await
}

/// The provisional password a recovery handed the owner.
fn recovered_password(reply: ServerMessage) -> String {
    match reply {
        ServerMessage::UserList { temp_password: Some(temp), users, .. } => {
            assert_eq!(users[0].recoveries.len(), 1, "it's recorded");
            temp
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn under_company_the_owner_recovers_alone_and_the_person_is_told() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let owner_key = set_policy(&mut owner, "company").await.expect("the first non-private policy makes the owner's key");
    let (mut ana, token, _) = ana_with_a_code(&hub).await;
    tool_said(chat(&mut ana, "WRITE notes/segredo.md", "conversa").await);

    // A wrong key opens nothing, and nothing is recorded.
    assert!(matches!(recover(&mut owner, "ana", &warden_bootstrap::recovery::generate_escrow_keypair().secret_text, None).await, ServerMessage::UserError { .. }));
    let temp = recovered_password(recover(&mut owner, "ana", &owner_key, None).await);

    // She signs in with the provisional password: told, and her data is intact once she picks her own.
    let (mut ana, _, user) = member(&hub, &temp, Some(token.clone())).await.unwrap();
    let user = user.unwrap();
    assert!(user.must_change_password && user.recovery_policy == "company" && user.member_policy == "company", "{user:?}");
    assert_eq!((user.recoveries.len(), user.recoveries[0].kind.as_str(), user.recoveries[0].seen), (1, "company", false));
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: temp.clone(), new_password: "anas-new-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { recovery_code: None, .. }));
    let said = tool_said(chat(&mut ana, "READ notes/segredo.md", "conversa").await);
    assert!(said.contains("written"), "{said}");
    ana.send(&ClientMessage::AckRecoveryNotices { request_id: 2 }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::RecoveryNoticesAcked { request_id: 2 }));
    let (_, _, user) = member(&hub, "anas-new-pass", Some(token)).await.unwrap();
    assert!(user.unwrap().recoveries[0].seen, "once seen, it stays as history");
}

#[tokio::test]
async fn under_consent_the_owner_needs_the_persons_code_as_well_as_the_key() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let owner_key = set_policy(&mut owner, "consent").await.unwrap();
    let (mut ana, token, code) = ana_with_a_code(&hub).await;
    tool_said(chat(&mut ana, "WRITE notes/segredo.md", "conversa").await);

    for (key, given) in [(owner_key.as_str(), None), (owner_key.as_str(), Some(warden_bootstrap::member_crypto::generate_recovery_code())), (&*warden_bootstrap::recovery::generate_escrow_keypair().secret_text, Some(code.clone()))] {
        assert!(matches!(recover(&mut owner, "ana", key, given.as_deref()).await, ServerMessage::UserError { .. }), "one half alone opens nothing");
    }
    let temp = recovered_password(recover(&mut owner, "ana", &owner_key, Some(&code)).await);

    let (mut ana, _, user) = member(&hub, &temp, Some(token)).await.unwrap();
    assert!(user.unwrap().must_change_password);
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: temp, new_password: "anas-new-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { .. }));
    let said = tool_said(chat(&mut ana, "READ notes/segredo.md", "conversa").await);
    assert!(said.contains("written"), "{said}");

    // The owner moves the workspace back to private: stronger, so it applies at her next sign-in on its
    // own — and leaving consent makes a new code, handed over right after the sign-in.
    set_policy(&mut owner, "private").await;
    let (mut again, _, user) = member(&hub, "anas-new-pass", None).await.unwrap();
    assert_eq!(user.unwrap().member_policy, "private", "the HelloAck already describes her data as the sign-in left it");
    match reply(&mut again).await {
        ServerMessage::RecoveryCode { request_id: 0, code } => assert_eq!(code.len(), 39),
        other => panic!("{other:?}"),
    }
    assert!(matches!(recover(&mut owner, "ana", &owner_key, Some(&code)).await, ServerMessage::UserError { message, .. } if message.contains("private")), "and now nobody can");
}

#[tokio::test]
async fn a_weaker_policy_waits_for_the_persons_yes_and_only_then_lets_the_owner_in() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (mut ana, token, _) = ana_with_a_code(&hub).await;
    tool_said(chat(&mut ana, "WRITE notes/segredo.md", "conversa").await);

    // The workspace goes to company after her data was made private: she's told, and nothing changes yet.
    let owner_key = set_policy(&mut owner, "company").await.unwrap();
    let (mut ana, _, user) = member(&hub, "anas-own-pass", Some(token.clone())).await.unwrap();
    let user = user.unwrap();
    assert_eq!((user.recovery_policy.as_str(), user.member_policy.as_str(), user.policy_pending), ("company", "private", true), "{user:?}");
    assert!(matches!(recover(&mut owner, "ana", &owner_key, None).await, ServerMessage::UserError { message, .. } if message.contains("private")), "her data is still private");

    // A wrong password can't say yes for her; hers does.
    ana.send(&ClientMessage::AcceptRecoveryPolicy { request_id: 1, password: "not-her-password".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    ana.send(&ClientMessage::AcceptRecoveryPolicy { request_id: 2, password: "anas-own-pass".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::RecoveryPolicyAccepted { request_id: 2, recovery_code: None }), "private to company needs no new code");
    let temp = recovered_password(recover(&mut owner, "ana", &owner_key, None).await);
    assert_ne!(temp, "anas-own-pass");
    let _ = tool_said(chat(&mut ana, "READ notes/segredo.md", "conversa").await); // her open connection still works

    // Company to consent is weaker still... no: consent is stronger than company, so it applies on its
    // own at the next sign-in, with a new code.
    set_policy(&mut owner, "consent").await;
    let (mut again, _, _) = member(&hub, &temp, None).await.unwrap();
    assert!(matches!(reply(&mut again).await, ServerMessage::RecoveryCode { request_id: 0, .. }), "entering consent hands out a new code");
}

// ---- Fatia 5: the invite to link a TruthID ----

fn word(value: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&value.to_be_bytes());
    out
}

/// What the registry answers for `getIdentity`: `(id, username, controller, exists)`.
fn registry_answer(id: u64, username: &str, exists: bool) -> String {
    let mut out = word(32).to_vec();
    out.extend_from_slice(&word(id));
    out.extend_from_slice(&word(128));
    out.extend_from_slice(&[0u8; 32]);
    out.extend_from_slice(&word(exists as u64));
    out.extend_from_slice(&word(username.len() as u64));
    out.extend_from_slice(username.as_bytes());
    out.resize(out.len() + (32 - username.len() % 32) % 32, 0);
    format!("0x{}", out.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// A JSON-RPC endpoint that answers every call with the same identity, standing in for Base.
async fn fake_registry(id: u64, username: &'static str, exists: bool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut request = vec![0u8; 8192];
                let _ = socket.read(&mut request).await;
                let body = format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{}"}}"#, registry_answer(id, username, exists));
                let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    url
}

fn point_truthid_at(hub: &Hub, rpc_url: &str) {
    let path = hub.dir.join("config.toml");
    let mut config = warden_bootstrap::load_config_from_path(&path, false).unwrap();
    config.truthid_rpc_url = Some(rpc_url.to_string());
    save_config(&path, &config).unwrap();
}

#[tokio::test]
async fn an_invite_links_a_members_truthid_once_and_only_to_them() {
    let hub = spin_up().await;
    point_truthid_at(&hub, &fake_registry(42, "ana.silva", true).await);
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();

    owner.send(&ClientMessage::CreateInvite { request_id: 1, pairing_key: "wrong".into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }));
    owner.send(&ClientMessage::CreateInvite { request_id: 2, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    let code = match reply(&mut owner).await {
        ServerMessage::UserList { invite_code: Some(code), users, .. } => {
            assert!(users[0].invite_open && users[0].truthid.is_empty());
            code
        }
        other => panic!("{other:?}"),
    };
    let on_disk = std::fs::read_to_string(hub.dir.join("config.toml")).unwrap();
    assert!(!on_disk.contains(code.split_once(':').unwrap().1), "only a hash of the secret is kept");

    // Ana signs in, picks her own password, and links.
    let (mut ana, _, _) = member(&hub, TEMP, None).await.unwrap();
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { .. }));
    ana.send(&ClientMessage::RedeemInvite { request_id: 3, code: "ana:not-the-secret".into(), username: "@ana.silva".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { .. }), "a wrong code links nothing");
    ana.send(&ClientMessage::RedeemInvite { request_id: 4, code: code.clone(), username: "@ana.silva".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::TruthIdLinked { username, .. } if username == "ana.silva"));
    ana.send(&ClientMessage::RedeemInvite { request_id: 5, code, username: "ana.silva".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { .. }), "single use");

    // A member can't make invites or untie anyone, and the owner sees the link and can undo it.
    ana.send(&ClientMessage::CreateInvite { request_id: 6, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    owner.send(&ClientMessage::ListUsers { request_id: 7 }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserList { users, .. } if users[0].truthid == "ana.silva" && !users[0].invite_open));
    owner.send(&ClientMessage::UnlinkTruthId { request_id: 8, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserList { users, .. } if users[0].truthid.is_empty()));
}

#[tokio::test]
async fn an_invite_for_a_truthid_that_does_not_exist_is_kept_for_another_try() {
    let hub = spin_up().await;
    point_truthid_at(&hub, &fake_registry(0, "", false).await);
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    owner.send(&ClientMessage::CreateInvite { request_id: 1, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    let ServerMessage::UserList { invite_code: Some(code), .. } = reply(&mut owner).await else { panic!("no invite") };
    let (mut ana, _, _) = member(&hub, TEMP, None).await.unwrap();
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { .. }));
    ana.send(&ClientMessage::RedeemInvite { request_id: 2, code, username: "nobody".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { message, .. } if message.contains("no TruthID named")));
    owner.send(&ClientMessage::ListUsers { request_id: 3 }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserList { users, .. } if users[0].invite_open && users[0].truthid.is_empty()));
}

#[tokio::test]
async fn the_owner_brings_back_a_removed_member_whose_encrypted_data_was_kept() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (mut ana, _, _) = ana_with_a_code(&hub).await;
    tool_said(chat(&mut ana, "WRITE notes/segredo.md", "conversa").await);

    owner.send(&ClientMessage::RemoveUser { request_id: 1, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::UserList { users, removed, .. } => {
            assert!(users.is_empty());
            assert_eq!(removed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["ana"], "kept with the key that opens her data");
        }
        other => panic!("{other:?}"),
    }
    assert!(member(&hub, "anas-own-pass", None).await.is_err(), "while removed she can't sign in");

    owner.send(&ClientMessage::RestoreUser { request_id: 2, pairing_key: "wrong".into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }));
    owner.send(&ClientMessage::RestoreUser { request_id: 3, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::UserList { users, removed, .. } => assert_eq!((users.len(), removed.len()), (1, 0)),
        other => panic!("{other:?}"),
    }

    // She signs in with the password she had, and what she wrote is still there.
    // Her old phone was revoked when she was removed, so it's a new device that pairs.
    assert!(member(&hub, "anas-own-pass", None).await.is_err(), "the revoked device stays revoked");
    let (mut again, _, _) =
        ServerConnection::handshake_as_member(&hub.url, "ana-new-phone", "Ana's new phone", "ana", "anas-own-pass", None, warden_server_protocol::tls::default_client_config()).await.unwrap();
    assert_eq!(ana_vault(&hub, "anas-own-pass").read("notes/segredo.md").unwrap(), "written");

    // A member can't restore anyone.
    again.send(&ClientMessage::RestoreUser { request_id: 4, pairing_key: KEY.into(), id: "ana".into() }).await.unwrap();
    assert!(matches!(reply(&mut again).await, ServerMessage::UserError { auth_rejected: true, .. }));
}

// ---- P113: signing in with a TruthID ----

/// A JSON-RPC endpoint that answers `getDevice(address)` from a table of `(address, identity, revoked)`;
/// an address that isn't in it doesn't exist. Standing in for the `DeviceRegistry` on Base.
async fn fake_device_registry(devices: Vec<(String, u64, bool)>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let devices = Arc::new(devices);
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let devices = devices.clone();
            tokio::spawn(async move {
                let mut request = vec![0u8; 8192];
                let read = socket.read(&mut request).await.unwrap_or(0);
                let text = String::from_utf8_lossy(&request[..read]).to_string();
                let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or_default();
                let data = serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v["params"][0]["data"].as_str().map(str::to_string)).unwrap_or_default();
                let asked = format!("0x{}", &data[data.len().saturating_sub(40)..]).to_lowercase();
                let (identity, revoked, exists) = devices.iter().find(|(a, _, _)| a.to_lowercase() == asked).map(|(_, id, revoked)| (*id, *revoked, true)).unwrap_or((0, false, false));
                let mut out = word(32).to_vec();
                out.extend_from_slice(&word(identity));
                out.extend_from_slice(&[0u8; 32]);
                out.extend_from_slice(&word(192));
                out.extend_from_slice(&word(1));
                out.extend_from_slice(&word(revoked as u64));
                out.extend_from_slice(&word(exists as u64));
                out.extend_from_slice(&word(0));
                let result = format!("0x{}", out.iter().map(|b| format!("{b:02x}")).collect::<String>());
                let reply = format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{result}"}}"#);
                let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}", reply.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    url
}

fn device_key(seed: u8) -> k256::ecdsa::SigningKey {
    k256::ecdsa::SigningKey::from_slice(&[seed; 32]).unwrap()
}

fn device_address(key: &k256::ecdsa::SigningKey) -> String {
    warden_truthid::login::address_of(key.verifying_key())
}

/// The hub knows its public https address and where the registry is, and Ana's TruthID is identity 42.
fn set_up_truthid(hub: &Hub, rpc_url: &str, public_url: Option<&str>) {
    let path = hub.dir.join("config.toml");
    let mut config = warden_bootstrap::load_config_from_path(&path, false).unwrap();
    config.truthid_rpc_url = Some(rpc_url.to_string());
    config.truthid_public_url = public_url.map(String::from);
    config.users[0].truthid = Some(warden_bootstrap::users::TruthIdLink { username: "ana.silva".into(), identity_id: 42, linked_at: 1 });
    save_config(&path, &config).unwrap();
}

/// A browser asking to sign in with a TruthID: what it gets back, frame by frame.
struct Browser {
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
}

impl Browser {
    async fn open(hub: &Hub, device: &str) -> Self {
        use futures_util::SinkExt;
        let (mut ws, _) = tokio_tungstenite::connect_async(&hub.url).await.unwrap();
        let hello = json!({ "type": "hello", "deviceId": device, "deviceName": "A browser", "authKey": "", "truthidLogin": true, "recoveryCodes": true });
        ws.send(tokio_tungstenite::tungstenite::Message::Text(hello.to_string().into())).await.unwrap();
        Self { ws }
    }

    async fn next(&mut self) -> serde_json::Value {
        use futures_util::StreamExt;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), self.ws.next()).await.expect("no frame").expect("closed").unwrap() {
                tokio_tungstenite::tungstenite::Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                tokio_tungstenite::tungstenite::Message::Close(_) => return json!({ "type": "closed" }),
                _ => continue,
            }
        }
    }

    /// The challenge the hub put in the QR, and the callback it named.
    async fn challenge(&mut self) -> (warden_truthid::login::AuthChallenge, String) {
        let frame = self.next().await;
        assert_eq!(frame["type"], "truthIdChallenge", "{frame}");
        let payload: serde_json::Value = serde_json::from_str(frame["payload"].as_str().unwrap()).unwrap();
        assert_eq!(payload["action"], "truthid-auth");
        (serde_json::from_value(payload["challenge"].clone()).unwrap(), payload["callbackUrl"].as_str().unwrap().to_string())
    }
}

async fn phone_posts(hub: &Hub, answer: &warden_truthid::login::AuthResponse) -> u16 {
    http(&hub.url, "POST", "/auth/truthid", "", Some(&serde_json::to_string(answer).unwrap())).await.0
}

#[tokio::test]
async fn a_member_signs_in_with_the_truthid_app_and_nobody_else_does() {
    let hub = spin_up().await;
    let phone = device_key(7);
    let revoked = device_key(8);
    let others = device_key(9);
    let stranger = device_key(10);
    let registry = fake_device_registry(vec![
        (device_address(&phone), 42, false),
        (device_address(&revoked), 42, true),
        (device_address(&others), 99, false), // a real TruthID that nobody linked here
    ])
    .await;
    set_up_truthid(&hub, &registry, Some("https://hub.test/"));

    // The right phone: the browser gets the QR, then is let in as Ana, paired, with no password.
    let mut browser = Browser::open(&hub, "browser-1").await;
    let (challenge, callback) = browser.challenge().await;
    assert_eq!((challenge.origin.as_str(), callback.as_str()), ("hub.test", "https://hub.test/auth/truthid"));
    assert_eq!(challenge.kind, "challenge");

    // Answers that aren't for a challenge nobody waits on, or are garbled, change nothing.
    let mut wrong_nonce = warden_truthid::login::sign_challenge(&phone, &challenge);
    wrong_nonce.nonce = "not-a-challenge".into();
    assert_eq!(phone_posts(&hub, &wrong_nonce).await, 400);
    assert_eq!(http(&hub.url, "POST", "/auth/truthid", "", Some("not json")).await.0, 400);
    assert_eq!(http(&hub.url, "GET", "/auth/truthid", "", None).await.0, 405);

    let answer = warden_truthid::login::sign_challenge(&phone, &challenge);
    assert_eq!(phone_posts(&hub, &answer).await, 200);
    let ack = browser.next().await;
    assert_eq!(ack["type"], "helloAck", "{ack}");
    assert_eq!((ack["user"]["id"].as_str(), ack["user"]["mustChangePassword"].as_bool().unwrap_or(false)), (Some("ana"), true), "she is still on her provisional password: {ack}");
    assert!(ack["deviceToken"].as_str().is_some_and(|t| !t.is_empty()), "a token to come back with: {ack}");
    // The same answer can't be used twice.
    assert_eq!(phone_posts(&hub, &answer).await, 400);

    // A device the registry revoked, one that isn't registered, one of an identity nobody linked, a
    // signature by another device than the one named, and a phone that declined: each is turned away.
    let mut cases: Vec<(&str, warden_truthid::login::AuthResponse)> = Vec::new();
    for (name, key) in [("revoked", &revoked), ("unregistered", &stranger), ("unlinked identity", &others)] {
        let mut b = Browser::open(&hub, &format!("browser-{name}")).await;
        let (c, _) = b.challenge().await;
        let answer = warden_truthid::login::sign_challenge(key, &c);
        assert_eq!(phone_posts(&hub, &answer).await, 401, "{name}");
        let frame = b.next().await;
        assert_eq!(frame["type"], "authError", "{name}: {frame}");
        cases.push((name, answer));
    }
    let mut b = Browser::open(&hub, "browser-impostor").await;
    let (c, _) = b.challenge().await;
    let mut impostor = warden_truthid::login::sign_challenge(&stranger, &c);
    impostor.device_address = device_address(&phone); // claims to be the registered phone
    assert_eq!(phone_posts(&hub, &impostor).await, 401);
    assert_eq!(b.next().await["type"], "authError");
    let mut b = Browser::open(&hub, "browser-declined").await;
    let (c, _) = b.challenge().await;
    let declined = warden_truthid::login::AuthResponse { approved: false, nonce: c.nonce.clone(), signature: String::new(), device_address: String::new() };
    assert_eq!(phone_posts(&hub, &declined).await, 401);
    assert_eq!(b.next().await["type"], "authError");
    assert_eq!(cases.len(), 3);

    // A browser that goes away leaves nothing waiting.
    let mut gone = Browser::open(&hub, "browser-gone").await;
    let (c, _) = gone.challenge().await;
    drop(gone);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(phone_posts(&hub, &warden_truthid::login::sign_challenge(&phone, &c)).await, 400, "nobody is waiting for it any more");
}

#[tokio::test]
async fn signing_in_with_a_truthid_needs_the_hub_to_know_its_https_address() {
    let hub = spin_up().await;
    let registry = fake_device_registry(Vec::new()).await;
    for public in [None, Some("http://hub.test"), Some("hub.test")] {
        set_up_truthid(&hub, &registry, public);
        let mut browser = Browser::open(&hub, "browser-x").await;
        let frame = browser.next().await;
        assert_eq!(frame["type"], "authError", "{public:?}: {frame}");
        assert!(frame["reason"].as_str().unwrap().contains("truthid_public_url"), "{frame}");
    }
}

#[tokio::test]
async fn a_truthid_session_opens_no_data_key_so_encrypted_data_waits_for_the_password() {
    let hub = spin_up().await;
    let (mut ana, _, _) = ana_with_a_code(&hub).await;
    tool_said(chat(&mut ana, "WRITE notes/segredo.md", "conversa").await);
    // The hub "restarts": it holds nobody's key.
    forget_anas_key(&hub);
    let phone = device_key(7);
    let registry = fake_device_registry(vec![(device_address(&phone), 42, false)]).await;
    set_up_truthid(&hub, &registry, Some("https://hub.test"));

    let mut browser = Browser::open(&hub, "browser-2").await;
    let (challenge, _) = browser.challenge().await;
    assert_eq!(phone_posts(&hub, &warden_truthid::login::sign_challenge(&phone, &challenge)).await, 200);
    let ack = browser.next().await;
    assert_eq!(ack["type"], "helloAck", "{ack}");
    assert_eq!((ack["user"]["encrypted"].as_bool(), ack["user"]["locked"].as_bool()), (Some(true), Some(true)), "in, but her data is shut: {ack}");

    // Her password opens it, as it does for anyone coming back after a restart.
    let (mut again, _, user) = member(&hub, "anas-own-pass", None).await.unwrap();
    assert!(!user.unwrap().locked);
    let said = tool_said(chat(&mut again, "READ notes/segredo.md", "conversa3").await);
    assert!(said.contains("written"), "{said}");
}

/// P104: `search_history` finds what the person said before, and only theirs.
#[tokio::test]
async fn search_history_reads_the_speakers_conversations_and_nobody_elses() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (mut ana, _, _) = ana_with_a_code(&hub).await;
    tool_said(chat(&mut ana, "minha senha do cofre e tulipa", "a1").await);
    tool_said(chat(&mut owner, "falei de futebol ontem", "o1").await);

    let hers = tool_said(chat(&mut ana, "SEARCH tulipa", "a2").await);
    assert!(hers.contains("a1") && hers.contains("tulipa"), "{hers}");
    let not_the_owners = tool_said(chat(&mut ana, "SEARCH futebol", "a2").await);
    assert!(!not_the_owners.contains("o1") && !not_the_owners.contains("futebol\""), "{not_the_owners}");
    assert!(not_the_owners.contains("\"results\":[]"), "{not_the_owners}");

    let his = tool_said(chat(&mut owner, "SEARCH futebol", "o2").await);
    assert!(his.contains("o1"), "{his}");
    let not_anas = tool_said(chat(&mut owner, "SEARCH tulipa", "o2").await);
    assert!(!not_anas.contains("a1") && not_anas.contains("\"results\":[]"), "{not_anas}");

    // Her data is shut when the hub no longer holds her key: the search says so instead of reading nothing.
    forget_anas_key(&hub);
    let (mut back, _, _) = member(&hub, "anas-own-pass", None).await.unwrap();
    let again = tool_said(chat(&mut back, "SEARCH tulipa", "a3").await);
    assert!(again.contains("a1"), "unlocked again by her password: {again}");
}

fn turn_learning_on(hub: &Hub) {
    let path = hub.dir.join("config.toml");
    let mut config = warden_bootstrap::load_config_from_path(&path, false).unwrap();
    config.learning = warden_bootstrap::learning::LearningSettings { enabled: true, ..Default::default() };
    save_config(&path, &config).unwrap();
}

async fn skills_of(conn: &mut ServerConnection, request_id: u64) -> Vec<warden_server_protocol::protocol::SkillDto> {
    conn.send(&ClientMessage::ListSkills { request_id }).await.unwrap();
    match reply(conn).await {
        ServerMessage::SkillList { skills, .. } => skills,
        other => panic!("{other:?}"),
    }
}

/// P104: after a correction the assistant suggests a skill — in the vault of the person who taught it, pending
/// until they accept it — and only when the owner turned learning on.
#[tokio::test]
async fn a_correction_becomes_a_pending_skill_in_the_persons_own_vault() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (mut ana, _, _) = ana_with_a_code(&hub).await;

    // Off by default: a correction teaches nothing.
    tool_said(chat(&mut ana, "Nao, sempre separe em Adicionado e Corrigido", "c0").await);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(skills_of(&mut ana, 1).await.is_empty(), "learning is off");

    turn_learning_on(&hub);
    tool_said(chat(&mut ana, "Escreva as notas da versao 2", "c1").await);
    tool_said(chat(&mut ana, "Nao, sempre separe em Adicionado, Corrigido e Removido", "c1").await);
    // The suggestion is written after the answer went out: give it a moment.
    let mut suggested = Vec::new();
    for round in 0..40 {
        suggested = skills_of(&mut ana, 10 + round).await;
        if !suggested.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(suggested.len(), 1, "{suggested:?}");
    let skill = &suggested[0];
    assert_eq!((skill.name.as_str(), skill.proposed, skill.source.as_deref()), ("release-notes-style", true, Some("c1")));
    assert!(skill.proposed_at.is_some() && skill.body.contains("Added, Fixed and Removed"));

    // It's in her encrypted vault, and on nobody else's side.
    let raw = ana_vault(&hub, "anas-own-pass").read("skills/release-notes-style.md").unwrap();
    assert!(raw.contains("proposed: true"), "{raw}");
    assert!(!hub.dir.join("users/ana/vault/skills").exists() || std::fs::read_dir(hub.dir.join("users/ana/vault/skills")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains("release")), "not legible on disk");
    assert!(skills_of(&mut owner, 99).await.is_empty(), "the owner's vault has nothing");

    // A turn with nothing to learn adds nothing.
    tool_said(chat(&mut ana, "obrigada", "c1").await);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(skills_of(&mut ana, 98).await.len(), 1);

    // Accepting is saving it without the flag; rejecting is deleting it.
    let accepted = warden_server_protocol::protocol::SkillDto { proposed: false, source: None, proposed_at: None, ..skill.clone() };
    ana.send(&ClientMessage::SaveSkill { request_id: 200, skill: accepted, overwrite: true }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::SkillOk { .. }));
    let after = skills_of(&mut ana, 201).await;
    assert!(!after[0].proposed && after[0].source.is_none(), "{after:?}");
    ana.send(&ClientMessage::DeleteSkill { request_id: 202, name: "release-notes-style".into() }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::SkillOk { .. }));
    assert!(skills_of(&mut ana, 203).await.is_empty());
}


/// P115: a member can opt out of the assistant learning from their conversations, which only narrows
/// what the owner turned on; the owner has no such switch over the wire.
#[tokio::test]
async fn a_member_can_opt_out_of_learning_and_back_in() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (mut ana, _, _) = ana_with_a_code(&hub).await;
    turn_learning_on(&hub);

    owner.send(&ClientMessage::SetLearning { request_id: 1, enabled: false }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { .. }), "the owner uses the config file");

    ana.send(&ClientMessage::SetLearning { request_id: 2, enabled: false }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::LearningSet { request_id: 2 }));
    let path = hub.dir.join("config.toml");
    let config = warden_bootstrap::load_config_from_path(&path, false).unwrap();
    assert!(config.users.iter().find(|u| u.id == "ana").unwrap().learning_opt_out);

    tool_said(chat(&mut ana, "Escreva as notas da versao 2", "o1").await);
    tool_said(chat(&mut ana, "Nao, sempre separe em Adicionado, Corrigido e Removido", "o1").await);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(skills_of(&mut ana, 3).await.is_empty(), "she opted out");

    // Signing in again tells her where the switch stands, and that the workspace has learning on.
    let (_, _, user) = member(&hub, "anas-own-pass", None).await.unwrap();
    let user = user.unwrap();
    assert!(user.learning_opt_out && user.learning_enabled, "{user:?}");

    ana.send(&ClientMessage::SetLearning { request_id: 4, enabled: true }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::LearningSet { request_id: 4 }));
    tool_said(chat(&mut ana, "Nao, sempre separe em Adicionado, Corrigido e Removido", "o1").await);
    let mut learned = Vec::new();
    for round in 0..40 {
        learned = skills_of(&mut ana, 10 + round).await;
        if !learned.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(learned.len(), 1, "back in: {learned:?}");
}

/// P115: a lesson that belongs in a skill the person already has becomes a pending change to it, which
/// applies when they accept it through the ordinary save every client makes.
#[tokio::test]
async fn a_correction_for_an_existing_skill_becomes_a_pending_change_that_applies_when_accepted() {
    let hub = spin_up().await;
    let (mut ana, _, _) = ana_with_a_code(&hub).await;
    turn_learning_on(&hub);
    let mine = warden_server_protocol::protocol::SkillDto {
        name: "release-notes-style".into(),
        description: "How to lay out release notes.".into(),
        body: "Group them under Added and Fixed.".into(),
        agents: Vec::new(),
        proposed: false,
        source: None,
        proposed_at: None,
        revises: None,
    };
    ana.send(&ClientMessage::SaveSkill { request_id: 1, skill: mine, overwrite: false }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::SkillOk { .. }));

    tool_said(chat(&mut ana, "Escreva as notas da versao 2", "r1").await);
    tool_said(chat(&mut ana, "Nao, sempre separe em Adicionado, Corrigido e Removido", "r1").await);
    let mut change = None;
    for round in 0..40 {
        change = skills_of(&mut ana, 10 + round).await.into_iter().find(|s| s.proposed);
        if change.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let change = change.expect("a change was suggested");
    assert_eq!((change.name.as_str(), change.revises.as_deref()), ("release-notes-style-revision", Some("release-notes-style")));
    let original = skills_of(&mut ana, 60).await.into_iter().find(|s| s.name == "release-notes-style").unwrap();
    assert_eq!(original.body, "Group them under Added and Fixed.", "nothing changes before she accepts");

    // A client that edits and saves it as still pending keeps it a change; one that accepts applies it.
    let still = warden_server_protocol::protocol::SkillDto { revises: None, ..change.clone() };
    ana.send(&ClientMessage::SaveSkill { request_id: 61, skill: still, overwrite: true }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::SkillOk { .. }));
    assert_eq!(skills_of(&mut ana, 62).await.into_iter().find(|s| s.proposed).unwrap().revises.as_deref(), Some("release-notes-style"));

    let accepted = warden_server_protocol::protocol::SkillDto { proposed: false, source: None, proposed_at: None, revises: None, ..change };
    ana.send(&ClientMessage::SaveSkill { request_id: 63, skill: accepted, overwrite: true }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::SkillOk { .. }));
    let after = skills_of(&mut ana, 64).await;
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(!after[0].proposed && after[0].body.contains("Added, Fixed and Removed"), "{after:?}");
}

/// P117: the owner lists, approves and denies the bots' pairing requests; approving puts the sender on the
/// bot's allow-list in `config.toml`, a wrong key or a member changes nothing.
#[tokio::test]
async fn the_owner_approves_and_denies_the_bots_pairing_requests() {
    use warden_bootstrap::bot_pairing::{BotPairing, Issued, TELEGRAM, WHATSAPP};
    let hub = spin_up().await;
    let config_path = hub.dir.join("config.toml");
    let store = BotPairing::beside(&config_path);
    let now = warden_bootstrap::bot_access::unix_now();
    let Issued::Fresh(telegram) = store.request(TELEGRAM, "42", "ana", now).unwrap() else { panic!("a fresh request") };
    let Issued::Fresh(whatsapp) = store.request(WHATSAPP, "5511999999999@s.whatsapp.net", "", now).unwrap() else { panic!("a fresh request") };
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let resolve = |request_id, key: &str, code: &str, approve| ClientMessage::ResolveBotPairing { request_id, pairing_key: key.into(), code: code.into(), approve, member: None };

    owner.send(&ClientMessage::ListBotPairings { request_id: 2 }).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::BotPairings { pairings, .. } => {
            assert_eq!(pairings.len(), 2);
            assert_eq!((pairings[0].channel.as_str(), pairings[0].sender.as_str(), pairings[0].label.as_str()), ("telegram", "42", "ana"));
            assert!(pairings[0].code.contains('-'), "shown as ABCD-EFGH");
        }
        other => panic!("{other:?}"),
    }

    owner.send(&resolve(3, "wrong", &telegram, true)).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }));
    assert!(warden_bootstrap::load_config_from_path(&config_path, false).unwrap().telegram.allowed_users.is_empty(), "a wrong key changes nothing");

    owner.send(&resolve(4, KEY, "NOPE-NOPE", true)).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: false, .. }), "an unknown code is refused");

    owner.send(&resolve(5, KEY, &telegram, true)).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::BotPairings { pairings, .. } => assert_eq!(pairings.len(), 1, "only the WhatsApp one is left"),
        other => panic!("{other:?}"),
    }
    assert_eq!(warden_bootstrap::load_config_from_path(&config_path, false).unwrap().telegram.allowed_users, [42]);

    owner.send(&resolve(6, KEY, &whatsapp, false)).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::BotPairings { pairings, .. } => assert!(pairings.is_empty()),
        other => panic!("{other:?}"),
    }
    assert!(warden_bootstrap::load_config_from_path(&config_path, false).unwrap().whatsapp.allowed_chats.is_empty(), "denying lets nobody in");

    // A member never sees or decides them.
    let (mut ana, _, _) = member(&hub, TEMP, None).await.unwrap();
    ana.send(&ClientMessage::ListBotPairings { request_id: 2 }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    ana.send(&resolve(3, KEY, &telegram, true)).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
}

/// P10: the owner tests a provider's key through the hub. A key saved on the hub is tested without the client ever
/// having it, a typed one replaces it, a wrong pairing key and a member are turned away, and what comes back is a word
/// and a sentence that carry neither the key nor what the provider said.
#[tokio::test]
async fn the_owner_tests_a_providers_key_and_a_member_cannot() {
    use std::io::{Read, Write};
    use warden_server_protocol::protocol::ProviderEditDto;

    // A provider that takes only the key `sk-good`, and echoes the key it was sent when it refuses.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let head = String::from_utf8_lossy(&buffer[..read]).to_lowercase();
            let (status, body) = if head.contains("bearer sk-good") { (200, "{}".to_string()) } else { (401, r#"{"error":"Incorrect API key provided: sk-bad"}"#.to_string()) };
            let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        }
    });

    let hub = spin_up().await;
    let config_path = hub.dir.join("config.toml");
    let mut config = warden_bootstrap::load_config_from_path(&config_path, false).unwrap();
    config.providers.push(warden_bootstrap::ProviderConfig { id: "local".into(), kind: warden_bootstrap::Provider::OpenaiCompatible, api_key: Some("sk-good".into()), base_url: Some(url.clone()), model: Some("m".into()), node: None });
    save_config(&config_path, &config).unwrap();
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let ask = |request_id, key: &str, secret: warden_server_protocol::protocol::SecretEdit| ClientMessage::TestProvider {
        request_id,
        pairing_key: key.into(),
        provider: ProviderEditDto { original_id: Some("local".into()), id: "local".into(), kind: "openai_compatible".into(), base_url: url.clone(), model: "m".into(), api_key: secret, node: String::new() },
    };
    let kind_of = |reply: ServerMessage| match reply {
        ServerMessage::ProviderTest { ok, kind, message, .. } => (ok, kind, message),
        other => panic!("a test result, got {other:?}"),
    };

    owner.send(&ask(80, KEY, warden_server_protocol::protocol::SecretEdit::Keep)).await.unwrap();
    let (ok, kind, _) = kind_of(reply(&mut owner).await);
    assert_eq!((ok, kind.as_str()), (true, "ok"), "the saved key, which the client never had");

    owner.send(&ask(81, KEY, warden_server_protocol::protocol::SecretEdit::Set("sk-bad".into()))).await.unwrap();
    let (ok, kind, message) = kind_of(reply(&mut owner).await);
    assert_eq!((ok, kind.as_str()), (false, "rejected"), "the typed key replaces the saved one");
    assert!(!message.contains("sk-bad") && !message.contains("sk-good") && !message.contains("Incorrect"), "{message}");

    owner.send(&ask(82, "wrong", warden_server_protocol::protocol::SecretEdit::Keep)).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }), "a wrong pairing key tests nothing");

    // A member never tests the hub's providers, whatever they know.
    let (mut ana, _, _) = member(&hub, TEMP, None).await.unwrap();
    ana.send(&ask(83, KEY, warden_server_protocol::protocol::SecretEdit::Keep)).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
}

/// What `ListBotPairings` answers: who is waiting, and who a chat may be approved as speaking as.
async fn bot_pairings(conn: &mut ServerConnection) -> (Vec<warden_server_protocol::protocol::BotPairingDto>, Vec<warden_server_protocol::protocol::BotMemberDto>) {
    conn.send(&ClientMessage::ListBotPairings { request_id: 70 }).await.unwrap();
    match reply(conn).await {
        ServerMessage::BotPairings { pairings, members, .. } => (pairings, members),
        other => panic!("{other:?}"),
    }
}

/// P117: the owner approves a pairing request as a member of the workspace. The list says who can be chosen and
/// whether the bots are linked to them; a refusal (no `[bot_hub]`, not linked, not a member) lets nobody in and
/// leaves the request waiting; once linked, the chat lands on the allow-list *and* in `members`; an approval
/// without a member, and a denial that names one, map nobody.
#[tokio::test]
async fn the_owner_approves_a_pairing_request_as_a_member_of_the_workspace() {
    use warden_bootstrap::bot_access::BotHubSettings;
    use warden_bootstrap::bot_hub::{HubTokens, Linked};
    use warden_bootstrap::bot_pairing::{BotPairing, Issued, TELEGRAM, WHATSAPP};
    let hub = spin_up().await;
    let config_path = hub.dir.join("config.toml");
    let store = BotPairing::beside(&config_path);
    let now = warden_bootstrap::bot_access::unix_now();
    let code = |channel: &str, sender: &str| match store.request(channel, sender, "", now).unwrap() {
        Issued::Fresh(code) => code,
        other => panic!("{other:?}"),
    };
    let (telegram, whatsapp, stranger) = (code(TELEGRAM, "42"), code(WHATSAPP, "5511999999999@lid"), code(TELEGRAM, "43"));
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let resolve = |request_id, code: &str, approve, member: Option<&str>| ClientMessage::ResolveBotPairing { request_id, pairing_key: KEY.into(), code: code.into(), approve, member: member.map(String::from) };
    let refused = |reply: ServerMessage| match reply {
        ServerMessage::UserError { message, auth_rejected: false, .. } => message,
        other => panic!("a refusal, got {other:?}"),
    };
    let config = || warden_bootstrap::load_config_from_path(&config_path, false).unwrap();

    let (pairings, members) = bot_pairings(&mut owner).await;
    assert_eq!(pairings.len(), 3);
    assert_eq!(members.iter().map(|m| (m.id.as_str(), m.name.as_str(), m.linked)).collect::<Vec<_>>(), [("ana", "Ana", false)], "the member exists, the bots aren't linked to her yet");

    // Each refusal says what to do, and changes nothing.
    owner.send(&resolve(71, &telegram, true, Some("ana"))).await.unwrap();
    assert!(refused(reply(&mut owner).await).contains("[bot_hub]"), "no hub set up yet");
    let mut with_hub = config();
    with_hub.bot_hub = Some(BotHubSettings { url: hub.url.replacen("http://", "ws://", 1) });
    save_config(&config_path, &with_hub).unwrap();
    owner.send(&resolve(72, &telegram, true, Some("ana"))).await.unwrap();
    assert!(refused(reply(&mut owner).await).contains("warden bots link ana"), "not linked yet");
    owner.send(&resolve(73, &telegram, true, Some("ghost"))).await.unwrap();
    assert!(refused(reply(&mut owner).await).contains("no member 'ghost'"), "not a member of the workspace");
    assert!(config().telegram.allowed_users.is_empty() && config().telegram.members.is_empty(), "a refusal lets nobody in");
    assert_eq!(bot_pairings(&mut owner).await.0.len(), 3, "and the request stays");

    // Linked, she can be chosen, and the chat speaks as her.
    HubTokens::beside(&config_path).set("ana", Linked { device_id: "warden-bot-ana".into(), device_token: "t".into() }).unwrap();
    assert!(bot_pairings(&mut owner).await.1[0].linked);
    owner.send(&resolve(74, &telegram, true, Some("ana"))).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::BotPairings { pairings, .. } => assert_eq!(pairings.len(), 2),
        other => panic!("{other:?}"),
    }
    assert_eq!(config().telegram.allowed_users, [42]);
    assert_eq!(config().telegram.member_for(42), Some("ana"));

    // Without a member it is the owner's assistant, as before; a denial ignores the member it was given.
    owner.send(&resolve(75, &whatsapp, true, None)).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::BotPairings { .. }));
    assert_eq!(config().whatsapp.allowed_chats, ["5511999999999@lid"]);
    assert!(config().whatsapp.members.is_empty(), "no member chosen, none mapped");
    owner.send(&resolve(76, &stranger, false, Some("ana"))).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::BotPairings { pairings, .. } => assert!(pairings.is_empty()),
        other => panic!("{other:?}"),
    }
    assert_eq!(config().telegram.allowed_users, [42], "denied, so nobody else got in");
    assert_eq!(config().telegram.members.len(), 1, "and nothing was mapped");
}

/// P115: the owner picks the model the assistant learns with for a member — a model the hub has, with
/// the pairing key — and the pick lands in `config.toml` and in what the owner sees.
#[tokio::test]
async fn the_owner_picks_the_learning_model_of_a_member() {
    let hub = spin_up().await;
    let config_path = hub.dir.join("config.toml");
    let mut config = warden_bootstrap::load_config_from_path(&config_path, false).unwrap();
    config.providers.push(warden_bootstrap::ProviderConfig { id: "cheap".into(), kind: warden_bootstrap::Provider::Gemini, api_key: None, base_url: None, model: None, node: None });
    save_config(&config_path, &config).unwrap();
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let pick = |request_id, key: &str, provider: Option<&str>| ClientMessage::SetUserLearningProvider { request_id, pairing_key: key.into(), id: "ana".into(), provider: provider.map(String::from) };

    owner.send(&pick(2, "wrong", Some("cheap"))).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }));
    owner.send(&pick(3, KEY, Some("ghost"))).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::UserError { auth_rejected: false, .. }), "a model the hub lacks is refused");
    assert_eq!(warden_bootstrap::load_config_from_path(&config_path, false).unwrap().users[0].learning_provider, None);

    owner.send(&pick(4, KEY, Some("cheap"))).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::UserList { users, .. } => assert_eq!(users[0].learning_provider.as_deref(), Some("cheap")),
        other => panic!("{other:?}"),
    }
    assert!(std::fs::read_to_string(&config_path).unwrap().contains("learning_provider = \"cheap\""));

    owner.send(&pick(5, KEY, None)).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::UserList { users, .. } => assert_eq!(users[0].learning_provider, None, "back to the workspace's"),
        other => panic!("{other:?}"),
    }
}

/// P119: the owner's bots, Telegram token, delegation/TruthID settings and everything that reaches the hub's
/// machine are the owner's. A member's settings screen gets them empty, and nothing of them in the JSON.
#[tokio::test]
async fn a_member_never_sees_the_owners_bots_token_or_machine_settings() {
    let hub = spin_up().await;
    let (mut ana, _token, _code) = ana_with_a_code(&hub).await;
    let config_path = hub.dir.join("config.toml");
    let mut config = warden_bootstrap::load_config_from_path(&config_path, false).unwrap();
    config.telegram.allowed_users = vec![424242];
    config.whatsapp.allowed_chats = vec!["5511987654321".into()];
    config.learning.bot_chats = vec!["telegram:424242".into()];
    config.api_keys.telegram_bot_token = Some("123456:OWNER-telegram-token".into());
    config.enable_shell = Some(true);
    config.generated_path = Some("/srv/owner-generated".into());
    config.mcp_servers = vec![warden_bootstrap::McpServerConfig::Stdio { name: "owner-notes".into(), command: "npx".into(), args: Vec::new(), env: [("TOKEN".to_string(), "owner-mcp-secret".to_string())].into() }];
    config.ssh_hosts = vec![warden_bootstrap::SshHostConfig { id: "owner-box".into(), host: "owner.example.com".into(), user: "root".into(), port: 22, identity_file: None, enabled: false, agents: Vec::new(), require_approval: false }];
    config.delegate_max_depth = Some(3);
    config.truthid_public_url = Some("https://owner-hub.example.com".into());
    save_config(&config_path, &config).unwrap();

    ana.send(&ClientMessage::RequestSettings { request_id: 9 }).await.unwrap();
    let ServerMessage::Settings { settings, .. } = reply(&mut ana).await else { panic!("her settings") };
    assert_eq!(settings.bots, Default::default(), "the owner's bot lists are not hers");
    assert!(!settings.telegram_token.set);
    assert_eq!(settings.advanced, Default::default());
    assert_eq!(settings.machine, Default::default());
    let json = serde_json::to_string(&settings).unwrap();
    for owner_only in ["424242", "5511987654321", "OWNER-telegram-token", "owner-notes", "owner-mcp-secret", "owner-box", "owner.example.com", "owner-generated", "owner-hub.example.com"] {
        assert!(!json.contains(owner_only), "'{owner_only}' reached a member's screen: {json}");
    }

    // The owner, on the same hub, does see it (the machine slice only as read-only: this hub wasn't started to allow edits).
    let mut owner = ServerConnection::connect(&hub.url, "owner-pc", "Owner's PC", KEY).await.unwrap();
    owner.send(&ClientMessage::RequestSettings { request_id: 10 }).await.unwrap();
    let ServerMessage::Settings { settings, .. } = reply(&mut owner).await else { panic!("the owner's settings") };
    assert_eq!(settings.bots.telegram_allowed_users, [424242]);
    assert!(settings.telegram_token.set && settings.machine.enable_shell);
    assert_eq!(settings.machine.mcp_servers[0].env_keys, ["TOKEN"]);
    assert!(!settings.machine.writable && settings.machine.blocked_reason.contains("--allow-machine-settings"));
    assert!(!serde_json::to_string(&settings).unwrap().contains("owner-mcp-secret"), "not even the owner's screen carries a secret value");
}

/// P117: a Telegram or WhatsApp chat that speaks as a member is answered by the hub as her. Linking takes
/// her password (a wrong one links nothing) and keeps only the device token; a turn lands in her vault and
/// her conversations, never the owner's; a second turn reuses the connection; when the hub no longer holds
/// her key the chat is told her data is locked; and once the owner revokes the bot's device the chat is told
/// it isn't connected any more.
#[tokio::test]
async fn a_bot_chat_that_speaks_as_a_member_is_answered_by_the_hub_with_her_data() {
    use warden_bootstrap::bot_access::BotHubSettings;
    use warden_bootstrap::bot_hub::{self, HubMemberChat, HubTokens, MemberChat, MemberReply};

    let hub = spin_up().await;
    let (mut ana, token, _code) = ana_with_a_code(&hub).await;
    let config_path = hub.dir.join("config.toml");
    let mut config = warden_bootstrap::load_config_from_path(&config_path, false).unwrap();
    config.bot_hub = Some(BotHubSettings { url: hub.url.clone() });
    save_config(&config_path, &config).unwrap();
    let chat = HubMemberChat::new(config_path.clone());

    // Nothing is linked yet: the chat is told so, and the owner's assistant is never the fallback.
    assert_eq!(chat.ask("ana", "telegram-42", "hello").await, MemberReply::Failed(bot_hub::NOT_LINKED_REPLY.into()));
    assert!(bot_hub::link(&config_path, &hub.url, "ana", "not-her-password").await.is_err(), "a wrong password links nothing");
    assert_eq!(HubTokens::beside(&config_path).get("ana").unwrap(), None);
    let user = bot_hub::link(&config_path, &hub.url, "ana", "anas-own-pass").await.unwrap();
    assert_eq!((user.id.as_str(), user.must_change_password), ("ana", false));
    let linked = HubTokens::beside(&config_path).get("ana").unwrap().expect("the token is kept");
    assert_eq!(linked.device_id, "warden-bot-ana");
    assert!(!std::fs::read_to_string(config_path.with_file_name("bot_hub.json")).unwrap().contains("anas-own-pass"), "the password is never kept");

    // A turn: the hub runs her agent, which writes to her vault, and the chat's conversation is hers.
    match chat.ask("ana", "telegram-42", "WRITE notes/from-telegram.md").await {
        MemberReply::Text { content, .. } => assert!(content.starts_with("tool said"), "{content}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ana_vault(&hub, "anas-own-pass").read("notes/from-telegram.md").unwrap(), "written");
    assert!(!hub.dir.join("vault/notes/from-telegram.md").exists(), "never the owner's vault");
    assert!(conversation_ids(&mut ana).await.contains(&"telegram-42".to_string()), "her conversation, under the chat's id");
    let mut owner = ServerConnection::connect(&hub.url, "owner-pc", "Owner's PC", KEY).await.unwrap();
    assert!(!conversation_ids(&mut owner).await.contains(&"telegram-42".to_string()), "not in the owner's list");

    // Another turn, on the same connection: a plain answer.
    assert_eq!(chat.ask("ana", "telegram-42", "hello").await, MemberReply::Text { content: "plain".into(), attachments: Vec::new() });

    // The hub restarts: her token still gets the bot in, but her data is shut and the chat hears it.
    forget_anas_key(&hub);
    match chat.ask("ana", "telegram-42", "hello again").await {
        MemberReply::Failed(text) => assert!(text.contains("locked"), "{text}"),
        other => panic!("{other:?}"),
    }
    let (_open, _, user) = member(&hub, "anas-own-pass", Some(token)).await.unwrap();
    assert!(!user.unwrap().locked, "she opens it again with her password, from her own device");

    // The owner revokes the bot's device: it can't speak as her any more.
    warden_server::PairingStore::new(hub.dir.join("devices.json")).revoke(&linked.device_id).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(chat.ask("ana", "telegram-42", "hello").await, MemberReply::Failed(bot_hub::NOT_LINKED_REPLY.into()));
}

fn project(id: &str, name: &str) -> warden_server_protocol::protocol::ProjectDto {
    warden_server_protocol::protocol::ProjectDto { id: id.into(), name: name.into(), description: String::new(), instructions: format!("Instructions of {name}."), workdir: None, code: false }
}

async fn save_project(conn: &mut ServerConnection, id: &str, name: &str) -> ServerMessage {
    conn.send(&ClientMessage::SaveProject { request_id: 20, project: project(id, name), overwrite: false }).await.unwrap();
    reply(conn).await
}

async fn project_names(conn: &mut ServerConnection) -> Vec<String> {
    conn.send(&ClientMessage::ListProjects { request_id: 21 }).await.unwrap();
    match reply(conn).await {
        ServerMessage::ProjectList { projects, .. } => projects.into_iter().map(|p| p.id).collect(),
        other => panic!("{other:?}"),
    }
}

async fn chat_in(conn: &mut ServerConnection, message: &str, conversation: &str, project: Option<&str>) -> ServerMessage {
    conn.send(&ClientMessage::Chat { message: message.into(), conversation_id: Some(conversation.into()), attachments: Vec::new(), agent_id: Some("helper".into()), project_id: project.map(String::from), workdir: None }).await.unwrap();
    reply(conn).await
}

/// P103 on a real hub: a project belongs to whoever made it, a conversation started in one is held to its folder —
/// what the agent writes lands there, what is outside can't be read, no shell and no history search are offered — and
/// the list tells which project each conversation is in.
#[tokio::test]
async fn projects_belong_to_each_person_and_a_project_conversation_is_held_to_its_folder() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (mut ana, _, _) = member(&hub, TEMP, None).await.unwrap();

    // On a provisional password the Projects screen is as shut as everything else.
    ana.send(&ClientMessage::ListProjects { request_id: 1 }).await.unwrap();
    match reply(&mut ana).await {
        ServerMessage::ProjectError { message, .. } => assert!(message.contains("your own password"), "{message}"),
        other => panic!("{other:?}"),
    }
    ana.send(&ClientMessage::ChangePassword { request_id: 2, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { .. }));

    // Each makes their own, and sees only theirs.
    assert!(matches!(save_project(&mut owner, "tax", "Tax return").await, ServerMessage::ProjectOk { .. }));
    assert!(matches!(save_project(&mut ana, "garden", "Garden").await, ServerMessage::ProjectOk { .. }));
    assert!(matches!(save_project(&mut owner, "tax", "Again").await, ServerMessage::ProjectError { .. }), "an id taken");
    assert_eq!(project_names(&mut owner).await, ["tax"]);
    assert_eq!(project_names(&mut ana).await, ["garden"]);
    assert!(hub.dir.join("vault/projects/tax/PROJECT.md").exists(), "the owner's is a folder of the vault");
    assert!(!hub.dir.join("users/ana/vault/projects").exists(), "and Ana's is encrypted on disk, name and all");

    // The owner starts conversations: one in the project, one outside it.
    chat_in(&mut owner, "WRITE outside.md", "plain", None).await;
    let answer = chat_in(&mut owner, "WRITE report.md", "in-tax", Some("tax")).await;
    assert!(matches!(answer, ServerMessage::ChatResponse { .. }), "{answer:?}");
    assert_eq!(std::fs::read_to_string(hub.dir.join("vault/projects/tax/report.md")).unwrap(), "written", "what the agent writes lands in the project's folder");
    assert!(!hub.dir.join("vault/report.md").exists());
    let offered = last_offered(&hub);
    assert!(offered.contains(&"write_file".to_string()) && !offered.contains(&"shell".to_string()) && !offered.contains(&"search_history".to_string()), "{offered:?}");
    // From inside, the rest of the vault isn't there.
    match chat_in(&mut owner, "READ outside.md", "in-tax", None).await {
        ServerMessage::ChatResponse { content, .. } => assert!(content.contains("tool said") && !content.contains("written"), "the note outside can't be read from the project: {content}"),
        other => panic!("{other:?}"),
    }
    // The project belongs to the conversation, not to what the client sends: this one named none and stayed in it.
    assert!(last_offered(&hub).contains(&"write_file".to_string()) && !last_offered(&hub).contains(&"shell".to_string()));
    // An ordinary conversation still has everything.
    chat_in(&mut owner, "READ outside.md", "plain", None).await;
    assert!(last_offered(&hub).contains(&"shell".to_string()));

    // A project that isn't there can't be started in.
    assert!(matches!(chat_in(&mut owner, "hi", "ghost", Some("nope")).await, ServerMessage::ChatError { message, .. } if message.contains("no project")));
    assert!(!conversation_ids(&mut owner).await.contains(&"ghost".to_string()));

    // The list says which project each conversation is in.
    owner.send(&ClientMessage::ListConversations { request_id: 30 }).await.unwrap();
    match reply(&mut owner).await {
        ServerMessage::ConversationList { conversations, .. } => {
            let project_of = |id: &str| conversations.iter().find(|c| c.id == id).unwrap().project_id.clone();
            assert_eq!((project_of("in-tax").as_deref(), project_of("plain")), (Some("tax"), None));
        }
        other => panic!("{other:?}"),
    }

    // Ana's turn in her project writes to her own (encrypted) vault, and she can't start one in the owner's.
    assert!(matches!(chat_in(&mut ana, "WRITE seeds.md", "ana-garden", Some("garden")).await, ServerMessage::ChatResponse { .. }));
    assert_eq!(ana_vault(&hub, "anas-own-pass").read("projects/garden/seeds.md").unwrap(), "written");
    assert!(matches!(chat_in(&mut ana, "hi", "ana-tax", Some("tax")).await, ServerMessage::ChatError { message, .. } if message.contains("no project")), "the owner's project isn't hers");
    assert!(!hub.dir.join("vault/projects/garden").exists());

    // Removing a project only unmarks it: the file stays, and the conversation goes on without one.
    owner.send(&ClientMessage::DeleteProject { request_id: 31, id: "tax".into() }).await.unwrap();
    assert!(matches!(reply(&mut owner).await, ServerMessage::ProjectOk { .. }));
    assert!(project_names(&mut owner).await.is_empty());
    assert!(hub.dir.join("vault/projects/tax/report.md").exists(), "the files stay as ordinary notes");
    chat_in(&mut owner, "hello", "in-tax", None).await;
    assert!(last_offered(&hub).contains(&"shell".to_string()), "the conversation is an ordinary one again");
}

async fn move_to(conn: &mut ServerConnection, conversation: &str, project: Option<&str>) -> ServerMessage {
    conn.send(&ClientMessage::MoveConversation { request_id: 40, conversation_id: conversation.into(), project_id: project.map(String::from) }).await.unwrap();
    reply(conn).await
}

async fn history_len(conn: &mut ServerConnection, conversation: &str) -> usize {
    conn.send(&ClientMessage::RequestHistory { request_id: 42, limit: None, conversation_id: Some(conversation.into()) }).await.unwrap();
    match reply(conn).await {
        ServerMessage::History { messages, .. } => messages.len(),
        other => panic!("{other:?}"),
    }
}

async fn project_of(conn: &mut ServerConnection, conversation: &str) -> Option<String> {
    conn.send(&ClientMessage::ListConversations { request_id: 41 }).await.unwrap();
    match reply(conn).await {
        ServerMessage::ConversationList { conversations, .. } => conversations.into_iter().find(|c| c.id == conversation).unwrap_or_else(|| panic!("no {conversation}")).project_id,
        other => panic!("{other:?}"),
    }
}

/// P103: a conversation moves into a project, between projects and out of one; the next turn runs in the new scope, what
/// was already said stays, and only a project of the person's own, on a conversation that isn't a task's, will do.
#[tokio::test]
async fn a_conversation_moves_between_projects_and_the_next_turn_runs_in_the_new_scope() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (mut ana, _, _) = member(&hub, TEMP, None).await.unwrap();

    // On a provisional password nothing moves, like everything else.
    match move_to(&mut ana, "x", Some("garden")).await {
        ServerMessage::ConversationError { message, .. } => assert!(message.contains("your own password"), "{message}"),
        other => panic!("{other:?}"),
    }
    ana.send(&ClientMessage::ChangePassword { request_id: 2, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { .. }));

    assert!(matches!(save_project(&mut owner, "tax", "Tax").await, ServerMessage::ProjectOk { .. }));
    assert!(matches!(save_project(&mut owner, "garden", "Garden").await, ServerMessage::ProjectOk { .. }));
    assert!(matches!(chat_in(&mut owner, "hi", "c1", None).await, ServerMessage::ChatResponse { .. }));
    assert!(last_offered(&hub).contains(&"shell".to_string()), "an ordinary conversation has the shell");

    // Into a project: the list says so, and the next turn — which names none — is held to it.
    assert!(matches!(move_to(&mut owner, "c1", Some("tax")).await, ServerMessage::ConversationOk { request_id: 40 }));
    assert_eq!(project_of(&mut owner, "c1").await.as_deref(), Some("tax"));
    assert!(matches!(chat_in(&mut owner, "WRITE after.md", "c1", None).await, ServerMessage::ChatResponse { .. }));
    assert!(hub.dir.join("vault/projects/tax/after.md").exists(), "what the agent writes now lands in the project");
    assert!(!last_offered(&hub).contains(&"shell".to_string()));
    assert_eq!(history_len(&mut owner, "c1").await, 4, "what was said before the move is still there");

    // Between projects, and back out.
    assert!(matches!(move_to(&mut owner, "c1", Some("garden")).await, ServerMessage::ConversationOk { .. }));
    chat_in(&mut owner, "WRITE g.md", "c1", None).await;
    assert!(hub.dir.join("vault/projects/garden/g.md").exists() && !hub.dir.join("vault/projects/tax/g.md").exists());
    assert!(matches!(move_to(&mut owner, "c1", None).await, ServerMessage::ConversationOk { .. }));
    assert_eq!(project_of(&mut owner, "c1").await, None);
    chat_in(&mut owner, "hello", "c1", None).await;
    assert!(last_offered(&hub).contains(&"shell".to_string()), "out of the project it has the shell again");

    // What can't be moved, and where.
    for (what, conversation, project) in [
        ("a project that isn't there", "c1", Some("nope")),
        ("a bad project id", "c1", Some("../x")),
        ("a conversation that isn't there", "ghost", Some("tax")),
        ("a scheduled task's conversation", "task-news", Some("tax")),
    ] {
        match move_to(&mut owner, conversation, project).await {
            ServerMessage::ConversationError { .. } => {}
            other => panic!("{what}: {other:?}"),
        }
    }
    assert_eq!(project_of(&mut owner, "c1").await, None, "a refused move changes nothing");
    assert_eq!(project_of(&mut owner, "task-news").await, None, "the task's conversation stayed out of every project");

    // Ana moves into her own project, never the owner's.
    assert!(matches!(save_project(&mut ana, "mine", "Mine").await, ServerMessage::ProjectOk { .. }));
    assert!(matches!(chat_in(&mut ana, "hi", "a1", None).await, ServerMessage::ChatResponse { .. }));
    assert!(matches!(move_to(&mut ana, "a1", Some("tax")).await, ServerMessage::ConversationError { message, .. } if message.contains("no project")), "the owner's project isn't hers");
    assert!(matches!(move_to(&mut ana, "a1", Some("mine")).await, ServerMessage::ConversationOk { .. }));
    assert_eq!(project_of(&mut ana, "a1").await.as_deref(), Some("mine"));
}

/// P103 b: a code project's folder is a path on the owner's machine, so a member who marks one of their own projects as
/// code (or gives it a working folder) still talks to the Warden's ordinary turn — never to the engine, and never with a
/// shell, which they don't have. Without the guard this hub, which has no engine, would answer with "no code engine".
#[tokio::test]
async fn a_member_never_gets_the_code_engine_or_a_shell_from_a_working_folder_they_name() {
    let hub = spin_up().await;
    let (mut ana, _, _) = member(&hub, TEMP, None).await.unwrap();
    ana.send(&ClientMessage::ChangePassword { request_id: 2, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::PasswordChanged { .. }));

    let mut code = project("repo", "Repo");
    code.workdir = Some("/etc".into());
    code.code = true;
    ana.send(&ClientMessage::SaveProject { request_id: 20, project: code, overwrite: false }).await.unwrap();
    assert!(matches!(reply(&mut ana).await, ServerMessage::ProjectOk { .. }));

    match chat_in(&mut ana, "hello", "a1", Some("repo")).await {
        ServerMessage::ChatResponse { content, .. } => assert_eq!(content, "plain"),
        other => panic!("{other:?}"),
    }
    assert!(!last_offered(&hub).contains(&"shell".to_string()), "no shell in a project, and she has none of her own");
}
