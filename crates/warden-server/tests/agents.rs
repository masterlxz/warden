//! P46 on the hub: a `Chat` names a configured agent, `manage_agents` asks the device that is
//! chatting for approval, and `message_agent` leaves a message in a conversation the device sees.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use warden_bootstrap::{load_config_from_path, save_config, AgentConfig, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::HistoryRole;

/// What one model call saw: its system messages and the tools it was offered.
#[derive(Debug, Clone)]
struct Seen {
    system: String,
    tools: Vec<String>,
}

/// Answers by what the turn is: a user message containing `CREATE` or `MESSAGE` asks for the matching
/// tool, a turn whose persona is the poet's writes a haiku, a tool result is echoed back.
struct Scripted {
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let system = messages.iter().filter(|m| m.role == Role::System).map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
        self.seen.lock().unwrap().push(Seen { system: system.clone(), tools: tools.iter().map(|t| t.name.clone()).collect() });
        let last = messages.last().unwrap();
        let reply = |content: String| Ok(response_stream(Response { content, tool_calls: Vec::new(), usage: None }));
        let call = |name: &str, arguments: serde_json::Value| {
            Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall { id: "call-1".into(), name: name.into(), arguments, thought_signature: None }],
                usage: None,
            }))
        };
        if last.role == Role::Tool {
            return reply(format!("tool said: {}", last.content));
        }
        if system.contains("You are a poet") {
            return reply("haiku!".into());
        }
        if last.content.contains("CREATE") {
            return call("manage_agents", json!({ "action": "create", "id": "critic", "persona": "You critique." }));
        }
        if last.content.contains("MESSAGE") {
            return call("message_agent", json!({ "action": "send", "agent_id": "poet", "message": "write a haiku", "wait": true }));
        }
        reply("plain".into())
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

fn agent(id: &str, persona: &str, manage: bool, message: bool) -> AgentConfig {
    AgentConfig {
        id: id.into(),
        persona: persona.into(),
        provider_id: None,
        can_delegate_to_agents: false,
        can_manage_agents: manage,
        can_message_agents: message,
        can_manage_tasks: false,
        allowed_tools: None,
        autonomy: warden_bootstrap::default_autonomy(),
        approval_required: Vec::new(),
        role: None,
        reports_to: None,
        owner: None,
        shared_with: Vec::new(),
        delegation_models: Vec::new(),
    }
}

struct Hub {
    url: String,
    config_path: PathBuf,
    seen: Arc<Mutex<Vec<Seen>>>,
}

async fn spin_up() -> Hub {
    let dir = std::env::temp_dir().join(format!(
        "warden-server-agents-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    let config = FileConfig {
        agents: vec![agent("chief", "You are the chief.", true, true), agent("poet", "You are a poet.", false, false)],
        ..FileConfig::default()
    };
    save_config(&config_path, &config).unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let orchestrator = Orchestrator::new(Arc::new(Scripted { seen: seen.clone() }), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(TestHost { path: config_path.clone() }));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}"), config_path, seen }
}

fn chat(message: &str, conversation_id: &str, agent_id: Option<&str>) -> ClientMessage {
    ClientMessage::Chat { message: message.into(), conversation_id: Some(conversation_id.into()), attachments: Vec::new(), agent_id: agent_id.map(str::to_string), project_id: None, workdir: None, thread_of: None }
}

/// Reads until the turn's `ChatResponse`/`ChatError`, answering every `ApprovalRequest` with
/// `approve`. Returns that reply and everything else that arrived before it.
async fn finish_turn(conn: &mut ServerConnection, approve: bool) -> (ServerMessage, Vec<ServerMessage>) {
    let mut others = Vec::new();
    loop {
        let msg = conn.recv().await.unwrap().expect("connection closed");
        match msg {
            ServerMessage::ChatResponse { .. } | ServerMessage::ChatError { .. } => return (msg, others),
            ServerMessage::ApprovalRequest { approval_id, .. } => {
                conn.send(&ClientMessage::ResolveApproval { approval_id, approved: approve, always: false }).await.unwrap();
                others.push(msg);
            }
            other => others.push(other),
        }
    }
}

#[tokio::test]
async fn a_chat_naming_an_agent_speaks_with_its_persona_and_the_conversation_remembers_it() {
    let hub = spin_up().await;
    let mut conn = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();

    conn.send(&chat("hello", "c1", Some("poet"))).await.unwrap();
    let (reply, _) = finish_turn(&mut conn, false).await;
    assert!(matches!(reply, ServerMessage::ChatResponse { ref content, .. } if content == "haiku!"), "got {reply:?}");
    let seen = hub.seen.lock().unwrap().last().cloned().unwrap();
    assert!(seen.system.contains("You are a poet."));
    // The poet has no opt-in flags, so none of the agent tools.
    assert!(!seen.tools.iter().any(|t| t == "manage_agents" || t == "message_agent"));

    conn.send(&ClientMessage::ListConversations { request_id: 1 }).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::ConversationList { conversations, .. }) => assert_eq!(conversations[0].agent_id.as_deref(), Some("poet")),
        other => panic!("expected ConversationList, got {other:?}"),
    }

    // The same conversation without an agent goes back to no persona.
    conn.send(&chat("hello", "c1", None)).await.unwrap();
    let (reply, _) = finish_turn(&mut conn, false).await;
    assert!(matches!(reply, ServerMessage::ChatResponse { ref content, .. } if content == "plain"), "got {reply:?}");
}

#[tokio::test]
async fn an_unknown_agent_is_an_error_before_the_model() {
    let hub = spin_up().await;
    let mut conn = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    conn.send(&chat("hello", "c1", Some("ghost"))).await.unwrap();
    let (reply, _) = finish_turn(&mut conn, false).await;
    match reply {
        ServerMessage::ChatError { message, conversation_id, .. } => {
            assert!(message.contains("agent 'ghost' not found"), "message was: {message}");
            assert_eq!(conversation_id.as_deref(), Some("c1"));
        }
        other => panic!("expected ChatError, got {other:?}"),
    }
    assert!(hub.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn manage_agents_asks_the_chatting_device_and_only_a_yes_saves() {
    for approve in [false, true] {
        let hub = spin_up().await;
        let mut conn = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
        conn.send(&chat("CREATE a critic", "c1", Some("chief"))).await.unwrap();
        let (reply, others) = finish_turn(&mut conn, approve).await;

        let asked: Vec<_> = others.iter().filter(|m| matches!(m, ServerMessage::ApprovalRequest { .. })).collect();
        assert_eq!(asked.len(), 1, "approve={approve}: {others:?}");
        let ServerMessage::ApprovalRequest { target, action, detail, .. } = asked[0] else { unreachable!() };
        assert_eq!((target.as_str(), action.as_str()), ("critic", "create_agent"));
        assert!(detail.contains("You critique."));
        assert!(matches!(reply, ServerMessage::ChatResponse { .. }), "got {reply:?}");

        let agents = load_config_from_path(&hub.config_path, true).unwrap().agents;
        assert_eq!(agents.iter().any(|a| a.id == "critic"), approve);
        // P120: the agent the chief creates reports to the chief.
        if approve {
            assert_eq!(agents.iter().find(|a| a.id == "critic").unwrap().reports_to.as_deref(), Some("chief"));
        }
    }
}

#[tokio::test]
async fn message_agent_leaves_a_message_the_device_can_open_and_the_colleague_answers_it() {
    let hub = spin_up().await;
    let mut conn = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    conn.send(&chat("MESSAGE the poet", "c1", Some("chief"))).await.unwrap();
    let (reply, others) = finish_turn(&mut conn, false).await;
    match &reply {
        ServerMessage::ChatResponse { content, .. } => assert!(content.contains("haiku!"), "content was: {content}"),
        other => panic!("expected ChatResponse, got {other:?}"),
    }

    // Told once when the message was saved and once when the poet answered.
    let changed: Vec<&str> = others
        .iter()
        .filter_map(|m| match m {
            ServerMessage::ConversationsChanged { conversation_id } => Some(conversation_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(changed.len(), 2, "{others:?}");
    let thread = changed[0].to_string();
    assert_eq!(thread, warden_bootstrap::message_agent::thread_id("chief", "poet"));

    conn.send(&ClientMessage::ListConversations { request_id: 1 }).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::ConversationList { conversations, .. }) => {
            let listed = conversations.iter().find(|c| c.id == thread).expect("the thread is listed");
            assert_eq!(listed.title, "chief → poet");
            assert_eq!(listed.agent_id.as_deref(), Some("poet"));
        }
        other => panic!("expected ConversationList, got {other:?}"),
    }
    conn.send(&ClientMessage::RequestHistory { request_id: 2, limit: None, conversation_id: Some(thread.clone()) }).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::History { messages, .. }) => {
            let turns: Vec<_> = messages.iter().map(|m| (m.role, m.content.as_str())).collect();
            assert_eq!(turns, vec![(HistoryRole::User, "Message from chief:\n\nwrite a haiku"), (HistoryRole::Assistant, "haiku!")]);
        }
        other => panic!("expected History, got {other:?}"),
    }

    // The poet answered as itself, without the tools that would let it message back.
    let seen = hub.seen.lock().unwrap().clone();
    let poet = seen.iter().find(|s| s.system.contains("You are a poet.")).expect("the poet was called");
    assert!(!poet.tools.iter().any(|t| t == "message_agent" || t == "manage_agents" || t == "delegate_to_agent"));
    let chief = seen.iter().find(|s| s.system.contains("You are the chief.")).unwrap();
    assert!(chief.tools.iter().any(|t| t == "message_agent") && chief.tools.iter().any(|t| t == "manage_agents"));

    // The device can now talk to the poet in that conversation, with the message as context.
    conn.send(&chat("thanks", &thread, Some("poet"))).await.unwrap();
    let (reply, _) = finish_turn(&mut conn, false).await;
    assert!(matches!(reply, ServerMessage::ChatResponse { ref content, .. } if content == "haiku!"));
}

fn set_autonomy(hub: &Hub, agent_id: &str, level: u8) {
    let mut config = load_config_from_path(&hub.config_path, true).unwrap();
    config.agents.iter_mut().find(|a| a.id == agent_id).unwrap().autonomy = level;
    save_config(&hub.config_path, &config).unwrap();
}

fn asked_actions(others: &[ServerMessage]) -> Vec<String> {
    others
        .iter()
        .filter_map(|m| match m {
            ServerMessage::ApprovalRequest { action, .. } => Some(action.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_agent_at_autonomy_three_asks_the_device_before_a_change_and_only_a_yes_runs_it() {
    for approve in [true, false] {
        let hub = spin_up().await;
        set_autonomy(&hub, "chief", 3);
        let mut conn = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
        conn.send(&chat("CREATE a critic", "c1", Some("chief"))).await.unwrap();
        let (reply, others) = finish_turn(&mut conn, approve).await;

        let saved = load_config_from_path(&hub.config_path, true).unwrap().agents.iter().any(|a| a.id == "critic");
        assert_eq!(saved, approve);
        if approve {
            // The level asks first, then `manage_agents` asks for what it will save.
            assert_eq!(asked_actions(&others), ["tool_call", "create_agent"]);
        } else {
            assert_eq!(asked_actions(&others), ["tool_call"]);
            assert!(matches!(&reply, ServerMessage::ChatResponse { content, .. } if content.contains("was not run")), "got {reply:?}");
        }
    }
}

#[tokio::test]
async fn an_agent_at_autonomy_five_creates_an_agent_without_asking_unless_the_kind_was_ticked() {
    use warden_core::autonomy::Category;
    // At 4 the tool asks for its own yes; at 5 nobody is asked; a ticked kind still asks before the tool runs, and then the tool
    // itself does not ask again.
    for (level, required, expected) in [
        (4, vec![], vec!["create_agent"]),
        (5, vec![], vec![]),
        (5, vec![Category::ElevatedAgent], vec!["tool_call"]),
    ] {
        let hub = spin_up().await;
        set_autonomy(&hub, "chief", level);
        set_approval_required(&hub, "chief", &required);
        let mut conn = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
        conn.send(&chat("CREATE a critic", "c1", Some("chief"))).await.unwrap();
        let (_, others) = finish_turn(&mut conn, true).await;
        assert_eq!(asked_actions(&others), expected, "level {level}, ticked {required:?}");
        let config = load_config_from_path(&hub.config_path, true).unwrap();
        let critic = config.agents.iter().find(|a| a.id == "critic").unwrap_or_else(|| panic!("level {level}: the agent was created"));
        // Whatever the level, the new agent is the cautious one, and the manager did not give it powers.
        assert_eq!((critic.autonomy, critic.can_manage_agents, critic.reports_to.as_deref()), (3, false, Some("chief")), "level {level}");
    }
}

#[tokio::test]
async fn an_agent_at_autonomy_two_only_suggests_and_one_has_no_tools() {
    let hub = spin_up().await;
    set_autonomy(&hub, "chief", 2);
    let mut conn = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    conn.send(&chat("CREATE a critic", "c1", Some("chief"))).await.unwrap();
    let (reply, others) = finish_turn(&mut conn, true).await;
    assert!(asked_actions(&others).is_empty(), "{others:?}");
    assert!(matches!(&reply, ServerMessage::ChatResponse { content, .. } if content.contains("only suggests")), "got {reply:?}");
    assert!(!load_config_from_path(&hub.config_path, true).unwrap().agents.iter().any(|a| a.id == "critic"));

    set_autonomy(&hub, "chief", 1);
    conn.send(&chat("hello", "c2", Some("chief"))).await.unwrap();
    finish_turn(&mut conn, true).await;
    let seen = hub.seen.lock().unwrap().last().cloned().unwrap();
    assert!(seen.tools.is_empty(), "level 1 is offered no tools: {:?}", seen.tools);
}

fn set_approval_required(hub: &Hub, agent_id: &str, categories: &[warden_core::autonomy::Category]) {
    let mut config = load_config_from_path(&hub.config_path, true).unwrap();
    config.agents.iter_mut().find(|a| a.id == agent_id).unwrap().approval_required = categories.to_vec();
    save_config(&hub.config_path, &config).unwrap();
}

#[tokio::test]
async fn an_agent_that_acts_alone_still_asks_for_the_kind_of_action_it_was_told_to_get_approved() {
    use warden_core::autonomy::Category;
    // Without the category the chief creates the agent after the tool's own yes only; with it, the device is asked first.
    for (required, expected) in [(vec![], vec!["create_agent"]), (vec![Category::ElevatedAgent], vec!["tool_call", "create_agent"])] {
        let hub = spin_up().await;
        set_approval_required(&hub, "chief", &required);
        let mut conn = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
        conn.send(&chat("CREATE a critic", "c1", Some("chief"))).await.unwrap();
        let (_, others) = finish_turn(&mut conn, true).await;
        assert_eq!(asked_actions(&others), expected, "required {required:?}");
        let by_rule = others.iter().find_map(|m| match m {
            ServerMessage::ApprovalRequest { action, category, .. } if action == "tool_call" => Some(category.clone()),
            _ => None,
        });
        assert_eq!(by_rule.flatten().as_deref(), if required.is_empty() { None } else { Some("elevated_agent") });
    }
}
