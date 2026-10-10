//! P92 on the hub: a scheduled task that is due runs as its agent, lands in its own conversation,
//! every connected device hears about it and lists it, and nobody is asked to approve anything.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use warden_bootstrap::tasks::TaskStore;
use warden_bootstrap::{load_config_from_path, save_config, AgentConfig, FileConfig, TaskConfig};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::{HistoryRole, TaskDto};

/// The tools each model call was offered, next to the system prompt it had.
type Offered = Arc<Mutex<Vec<(String, Vec<String>)>>>;

/// A poet writes a haiku; `CREATE` asks for `manage_agents`, `SCHEDULE` for `manage_tasks`; a tool
/// result is echoed back. Records the tools each call was offered, by the persona that got them.
struct Scripted {
    calls: Arc<Mutex<usize>>,
    offered: Offered,
}

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        *self.calls.lock().unwrap() += 1;
        let system = messages.iter().filter(|m| m.role == Role::System).map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
        self.offered.lock().unwrap().push((system.clone(), tools.iter().map(|t| t.name.clone()).collect()));
        let last = messages.last().unwrap();
        let reply = |content: String| Ok(response_stream(Response { content, tool_calls: Vec::new(), usage: None }));
        if last.role == Role::Tool {
            return reply(format!("tool said: {}", last.content));
        }
        if last.content.contains("SCHEDULE") {
            return Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "manage_tasks".into(),
                    arguments: json!({ "action": "create", "id": "digest", "agent_id": "poet", "prompt": "write a haiku", "cron": "0 8 * * 1-5", "timezone": "UTC" }),
                    thought_signature: None,
                }],
                usage: None,
            }));
        }
        if last.content.contains("CREATE") {
            return Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "manage_agents".into(),
                    arguments: json!({ "action": "create", "id": "critic", "persona": "You critique." }),
                    thought_signature: None,
                }],
                usage: None,
            }));
        }
        if system.contains("You are a poet") {
            return reply("haiku!".into());
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

fn agent(id: &str, persona: &str, manage: bool) -> AgentConfig {
    AgentConfig {
        id: id.into(),
        persona: persona.into(),
        provider_id: None,
        can_delegate_to_agents: false,
        can_manage_agents: manage,
        can_message_agents: false,
        can_manage_tasks: manage,
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
        can_message_user: true,
        can_choose_models: true,
    }
}

fn task(id: &str, agent: &str, prompt: &str) -> TaskConfig {
    TaskConfig {
        id: id.into(),
        agent: Some(agent.into()),
        prompt: prompt.into(),
        every: Some("1h".into()),
        cron: None,
        once: None,
        timezone: None,
        enabled: true,
    }
}

struct Hub {
    url: String,
    config_path: PathBuf,
    store: TaskStore,
    calls: Arc<Mutex<usize>>,
    offered: Offered,
}

impl Hub {
    /// Adds `tasks` to the config as if this hub had first seen them two hours ago — so an hourly
    /// one is due at the next tick.
    fn add_overdue_tasks(&self, tasks: Vec<TaskConfig>) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        self.store.claim_due(&tasks, now - 2 * 3_600_000).unwrap();
        let mut config = load_config_from_path(&self.config_path, true).unwrap();
        config.tasks = tasks;
        save_config(&self.config_path, &config).unwrap();
    }
}

async fn spin_up(run_tasks: bool) -> Hub {
    let dir = std::env::temp_dir().join(format!(
        "warden-server-tasks-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    let config = FileConfig { agents: vec![agent("chief", "You are the chief.", true), agent("poet", "You are a poet.", false)], ..FileConfig::default() };
    save_config(&config_path, &config).unwrap();

    let calls = Arc::new(Mutex::new(0));
    let offered: Offered = Arc::default();
    let orchestrator = Orchestrator::new(Arc::new(Scripted { calls: calls.clone(), offered: offered.clone() }), Arc::new(Vault::new(dir.join("vault"))));
    let store = TaskStore::new(dir.join("tasks"));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(TestHost { path: config_path.clone() }))
        .with_tasks(store.clone(), run_tasks)
        .with_task_tick(Duration::from_millis(50));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}"), config_path, store, calls, offered }
}

/// Every `ConversationsChanged` until `wanted` ones have arrived.
async fn changes(conn: &mut ServerConnection, wanted: usize) -> Vec<String> {
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while seen.len() < wanted {
            if let Some(ServerMessage::ConversationsChanged { conversation_id }) = conn.recv().await.unwrap() {
                seen.push(conversation_id);
            }
        }
    })
    .await
    .expect("the tasks ran");
    seen.sort();
    seen
}

async fn history(conn: &mut ServerConnection, conversation_id: &str) -> Vec<(HistoryRole, String)> {
    conn.send(&ClientMessage::RequestHistory { request_id: 9, limit: None, conversation_id: Some(conversation_id.into()) }).await.unwrap();
    loop {
        match conn.recv().await.unwrap() {
            Some(ServerMessage::History { messages, .. }) => return messages.into_iter().map(|m| (m.role, m.content)).collect(),
            Some(ServerMessage::ConversationsChanged { .. }) => continue,
            other => panic!("expected History, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_due_task_runs_as_its_agent_and_every_device_sees_its_conversation() {
    let hub = spin_up(true).await;
    let mut web = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    let mut phone = ServerConnection::connect(&hub.url, "phone-1", "Phone", "test-key").await.unwrap();
    hub.add_overdue_tasks(vec![task("poem", "poet", "write about the sea"), task("hire", "chief", "CREATE a critic")]);

    assert_eq!(changes(&mut web, 2).await, ["task-hire", "task-poem"]);
    assert_eq!(changes(&mut phone, 2).await, ["task-hire", "task-poem"]);

    phone.send(&ClientMessage::ListConversations { request_id: 1 }).await.unwrap();
    match phone.recv().await.unwrap() {
        Some(ServerMessage::ConversationList { conversations, .. }) => {
            let poem = conversations.iter().find(|c| c.id == "task-poem").expect("listed on every device");
            assert_eq!((poem.title.as_str(), poem.agent_id.as_deref()), ("Tarefa: poem", Some("poet")));
        }
        other => panic!("expected ConversationList, got {other:?}"),
    }

    let poem = history(&mut web, "task-poem").await;
    assert_eq!(poem.len(), 2);
    assert_eq!(poem[0].0, HistoryRole::User);
    assert!(poem[0].1.starts_with("[Scheduled task 'poem', ") && poem[0].1.ends_with("write about the sea"), "{}", poem[0].1);
    assert_eq!(poem[1], (HistoryRole::Assistant, "haiku!".to_string()));

    // Nobody was there to say yes, so `manage_agents` refused and nothing was saved.
    let hire = history(&mut web, "task-hire").await;
    assert!(hire[1].1.starts_with("tool said:"), "{}", hire[1].1);
    assert!(!load_config_from_path(&hub.config_path, true).unwrap().agents.iter().any(|a| a.id == "critic"));

    let states = hub.store.states().unwrap();
    assert!(states["poem"].last_finished_at_ms.is_some() && states["poem"].last_error.is_none());

    // The person can go on talking in the task's conversation.
    web.send(&ClientMessage::Chat { message: "another".into(), conversation_id: Some("task-poem".into()), attachments: Vec::new(), agent_id: Some("poet".into()), project_id: None, workdir: None, thread_of: None })
        .await
        .unwrap();
    loop {
        match web.recv().await.unwrap() {
            Some(ServerMessage::ChatResponse { content, .. }) => break assert_eq!(content, "haiku!"),
            Some(ServerMessage::ConversationsChanged { .. }) => continue,
            other => panic!("expected ChatResponse, got {other:?}"),
        }
    }
    assert_eq!(history(&mut web, "task-poem").await.len(), 4);
}

#[tokio::test]
async fn a_hub_without_run_tasks_runs_nothing() {
    let hub = spin_up(false).await;
    hub.add_overdue_tasks(vec![task("poem", "poet", "write about the sea")]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(*hub.calls.lock().unwrap(), 0);
    assert!(hub.store.states().unwrap()["poem"].last_run_at_ms.is_none());
}

/// The reply to a task request, skipping whatever else arrives meanwhile.
async fn task_reply(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            msg @ (ServerMessage::TaskList { .. } | ServerMessage::TaskError { .. }) => return msg,
            _ => continue,
        }
    }
}

fn dto(id: &str, agent: &str, prompt: &str) -> TaskDto {
    TaskDto { id: id.into(), agent_id: Some(agent.into()), prompt: prompt.into(), every: Some("1d".into()), cron: None, once: None, timezone: None, enabled: true }
}

#[tokio::test]
async fn the_web_manages_tasks_with_the_pairing_key() {
    let hub = spin_up(false).await;
    let mut web = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    let mut phone = ServerConnection::connect(&hub.url, "phone-1", "Phone", "test-key").await.unwrap();

    web.send(&ClientMessage::ListTasks { request_id: 1 }).await.unwrap();
    assert_eq!(task_reply(&mut web).await, ServerMessage::TaskList { request_id: 1, tasks: Vec::new(), runs_here: false });

    let save = |key: &str, original: Option<&str>, task: TaskDto| ClientMessage::SaveTask { request_id: 2, pairing_key: key.into(), original_id: original.map(str::to_string), task };
    web.send(&save("wrong", None, dto("poem", "poet", "write"))).await.unwrap();
    assert!(matches!(task_reply(&mut web).await, ServerMessage::TaskError { auth_rejected: true, .. }));
    assert!(load_config_from_path(&hub.config_path, true).unwrap().tasks.is_empty());

    web.send(&save("test-key", None, dto("poem", "ghost", "write"))).await.unwrap();
    match task_reply(&mut web).await {
        ServerMessage::TaskError { message, auth_rejected: false, .. } => assert!(message.contains("ghost"), "{message}"),
        other => panic!("expected TaskError, got {other:?}"),
    }

    web.send(&save("test-key", None, dto("poem", "poet", "write about the sea"))).await.unwrap();
    match task_reply(&mut web).await {
        ServerMessage::TaskList { tasks, runs_here: false, .. } => {
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].task.id, "poem");
            assert!(tasks[0].next_run_at_ms.is_some() && tasks[0].last_run_at_ms.is_none());
        }
        other => panic!("expected TaskList, got {other:?}"),
    }
    assert_eq!(load_config_from_path(&hub.config_path, true).unwrap().tasks[0].prompt, "write about the sea");

    // Rename it, pause it: the config follows.
    web.send(&save("test-key", Some("poem"), dto("sea", "poet", "write about the sea"))).await.unwrap();
    task_reply(&mut web).await;
    web.send(&ClientMessage::SetTaskEnabled { request_id: 3, pairing_key: "test-key".into(), id: "sea".into(), enabled: false }).await.unwrap();
    task_reply(&mut web).await;
    let saved = load_config_from_path(&hub.config_path, true).unwrap().tasks;
    assert_eq!((saved[0].id.as_str(), saved[0].enabled), ("sea", false));

    // Run now, on a hub that doesn't run tasks on schedule: every device hears when it's done.
    web.send(&ClientMessage::RunTask { request_id: 4, pairing_key: "test-key".into(), id: "sea".into() }).await.unwrap();
    assert!(matches!(task_reply(&mut web).await, ServerMessage::TaskList { request_id: 4, .. }));
    assert_eq!(changes(&mut phone, 1).await, ["task-sea"]);
    assert_eq!(history(&mut phone, "task-sea").await[1], (HistoryRole::Assistant, "haiku!".to_string()));
    web.send(&ClientMessage::ListTasks { request_id: 5 }).await.unwrap();
    match task_reply(&mut web).await {
        ServerMessage::TaskList { tasks, .. } => assert!(tasks[0].last_finished_at_ms.is_some() && !tasks[0].running && tasks[0].last_error.is_none()),
        other => panic!("expected TaskList, got {other:?}"),
    }

    web.send(&ClientMessage::DeleteTask { request_id: 6, pairing_key: "test-key".into(), id: "sea".into() }).await.unwrap();
    assert!(matches!(task_reply(&mut web).await, ServerMessage::TaskList { ref tasks, .. } if tasks.is_empty()));
    web.send(&ClientMessage::RunTask { request_id: 7, pairing_key: "test-key".into(), id: "sea".into() }).await.unwrap();
    assert!(matches!(task_reply(&mut web).await, ServerMessage::TaskError { auth_rejected: false, .. }));
}

#[tokio::test]
async fn an_agent_with_the_flag_schedules_a_task_once_the_device_says_yes() {
    for approve in [false, true] {
        let hub = spin_up(false).await;
        let mut web = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
        web.send(&ClientMessage::Chat { message: "SCHEDULE a daily haiku".into(), conversation_id: Some("c1".into()), attachments: Vec::new(), agent_id: Some("chief".into()), project_id: None, workdir: None, thread_of: None })
            .await
            .unwrap();
        let mut asked = Vec::new();
        let reply = loop {
            match web.recv().await.unwrap().expect("connection closed") {
                ServerMessage::ApprovalRequest { approval_id, target, action, detail, .. } => {
                    asked.push((target, action, detail));
                    web.send(&ClientMessage::ResolveApproval { approval_id, approved: approve, always: false }).await.unwrap();
                }
                msg @ (ServerMessage::ChatResponse { .. } | ServerMessage::ChatError { .. }) => break msg,
                _ => continue,
            }
        };
        assert!(matches!(reply, ServerMessage::ChatResponse { .. }), "{reply:?}");
        assert_eq!(asked.len(), 1, "approve={approve}");
        let (target, action, detail) = &asked[0];
        assert_eq!((target.as_str(), action.as_str()), ("digest", "create_task"));
        assert!(detail.contains("Agent: poet") && detail.contains("cron 0 8 * * 1-5 (UTC)") && detail.contains("write a haiku"), "{detail}");

        web.send(&ClientMessage::ListTasks { request_id: 1 }).await.unwrap();
        match task_reply(&mut web).await {
            ServerMessage::TaskList { tasks, .. } => assert_eq!(tasks.iter().map(|t| t.task.id.as_str()).collect::<Vec<_>>(), if approve { vec!["digest"] } else { vec![] }),
            other => panic!("expected TaskList, got {other:?}"),
        }

        // Only the agent with the flag was offered the tool.
        let offered = hub.offered.lock().unwrap().clone();
        assert!(offered.iter().any(|(system, tools)| system.contains("You are the chief.") && tools.iter().any(|t| t == "manage_tasks")));
    }

    let hub = spin_up(false).await;
    let mut web = ServerConnection::connect(&hub.url, "web-1", "Browser", "test-key").await.unwrap();
    web.send(&ClientMessage::Chat { message: "hello".into(), conversation_id: Some("c2".into()), attachments: Vec::new(), agent_id: Some("poet".into()), project_id: None, workdir: None, thread_of: None }).await.unwrap();
    loop {
        if let Some(ServerMessage::ChatResponse { .. }) = web.recv().await.unwrap() {
            break;
        }
    }
    let offered = hub.offered.lock().unwrap().clone();
    assert!(offered.iter().all(|(_, tools)| !tools.iter().any(|t| t == "manage_tasks")));
}
