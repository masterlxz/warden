//! P103 (b) on a real hub: a code project's conversation is a task for the code engine. The engine here is a script
//! (the opencode itself is covered in `warden-core`); what is checked is the hub around it — the events that reach the
//! client as the task runs, the approval that goes to the person and back, the session the conversation keeps, the stop
//! that finds the running task, and who may not have any of it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use warden_core::code_engine::{CodeEngine, CodeEvent, CodeMode, ToolEvent, ToolStatus, TurnOutcome, TurnRequest};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::{ApprovalRequest, Approver, ToolSpec};
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage};
use warden_server_protocol::protocol::{ChatEventDto, ProjectDto, ToolStatusDto};

const KEY: &str = "test-key";

struct Plain;
#[async_trait]
impl ModelProvider for Plain {
    async fn chat_stream(&self, _: Vec<Message>, _: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        Ok(response_stream(Response { content: "ordinary turn".into(), tool_calls: Vec::new(), usage: None }))
    }
}

/// A task that opens `ses_1`, uses one tool that needs a yes, and answers "done" — or, for "HANG", waits to be stopped.
struct Scripted {
    /// The mode of each task, as a watch the test can look at again later (a change in the middle of a task).
    modes: Mutex<Vec<tokio::sync::watch::Receiver<CodeMode>>>,
    asked: Mutex<Vec<TurnRequest>>,
    approvals: Mutex<Vec<bool>>,
    aborted: Mutex<Vec<(String, String)>>,
    stop: Notify,
}

#[async_trait]
impl CodeEngine for Scripted {
    async fn run_turn(&self, request: TurnRequest, approver: Option<Arc<dyn Approver>>, on_event: &mut (dyn FnMut(CodeEvent) + Send)) -> anyhow::Result<TurnOutcome> {
        let hang = request.prompt == "HANG";
        self.modes.lock().unwrap().push(request.mode.clone());
        self.asked.lock().unwrap().push(request);
        on_event(CodeEvent::Session("ses_1".into()));
        let tool = |status| CodeEvent::Tool(ToolEvent { call_id: "k1".into(), tool: "bash".into(), title: "cargo test".into(), status });
        on_event(tool(ToolStatus::Running));
        if hang {
            self.stop.notified().await;
            return Ok(TurnOutcome { session_id: "ses_1".into(), text: "stopped half way".into(), tools_used: vec!["bash".into()] });
        }
        let allowed = match approver {
            Some(approver) => approver.approve(ApprovalRequest { target: "Repo".into(), action: "bash".into(), detail: "cargo test".into() }).await,
            None => false,
        };
        self.approvals.lock().unwrap().push(allowed);
        on_event(tool(if allowed { ToolStatus::Completed } else { ToolStatus::Failed }));
        on_event(CodeEvent::Text("done".into()));
        Ok(TurnOutcome { session_id: "ses_1".into(), text: "done".into(), tools_used: vec!["bash".into()] })
    }

    async fn abort(&self, workdir: &str, session_id: &str) -> anyhow::Result<()> {
        self.aborted.lock().unwrap().push((workdir.to_string(), session_id.to_string()));
        self.stop.notify_one();
        Ok(())
    }
}

fn engine() -> Arc<Scripted> {
    Arc::new(Scripted { modes: Mutex::default(), asked: Mutex::default(), approvals: Mutex::default(), aborted: Mutex::default(), stop: Notify::new() })
}

struct Hub {
    url: String,
}

async fn spin_up(engine: Option<Arc<Scripted>>) -> Hub {
    std::env::set_var(warden_core::memory::embed::OFF_SWITCH, "1");
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("warden-server-code-{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let orchestrator = Orchestrator::new(Arc::new(Plain), Arc::new(Vault::new(dir.join("vault"))));
    let mut server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json")).await.unwrap().with_users_dir(dir.join("users"));
    if let Some(engine) = engine {
        server = server.with_code_engine(engine);
    }
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}") }
}

async fn owner(hub: &Hub) -> ServerConnection {
    ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap()
}

async fn next(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), conn.recv()).await.expect("no message").unwrap().expect("connection closed") {
            ServerMessage::ConversationsChanged { .. } | ServerMessage::Pong { .. } => continue,
            msg => return msg,
        }
    }
}

async fn save_project(conn: &mut ServerConnection, id: &str, workdir: Option<&str>, code: bool) {
    let project = ProjectDto { id: id.into(), name: "Repo".into(), description: String::new(), instructions: "Use tabs.".into(), workdir: workdir.map(str::to_string), code };
    conn.send(&ClientMessage::SaveProject { request_id: 1, project, overwrite: false }).await.unwrap();
    assert!(matches!(next(conn).await, ServerMessage::ProjectOk { .. }));
}

async fn say(conn: &mut ServerConnection, text: &str, project: Option<&str>) {
    conn.send(&ClientMessage::Chat { message: text.into(), conversation_id: Some("c1".into()), attachments: Vec::new(), agent_id: None, project_id: project.map(str::to_string), workdir: None }).await.unwrap();
}

#[tokio::test]
async fn a_code_projects_conversation_runs_on_the_engine_with_events_an_approval_and_a_kept_session() {
    let engine = engine();
    let hub = spin_up(Some(engine.clone())).await;
    let mut me = owner(&hub).await;
    save_project(&mut me, "repo", Some("/home/me/repo"), true).await;

    say(&mut me, "add a test", Some("repo")).await;
    // The work as it happens: the tool starts…
    match next(&mut me).await {
        ServerMessage::ChatEvent { conversation_id, event: ChatEventDto::Tool { tool, title, status, .. } } => {
            assert_eq!((conversation_id.as_str(), tool.as_str(), title.as_str(), status), ("c1", "bash", "cargo test", ToolStatusDto::Running));
        }
        other => panic!("{other:?}"),
    }
    // …the engine wants a yes for it, and the person's answer is what it gets…
    let approval_id = match next(&mut me).await {
        ServerMessage::ApprovalRequest { approval_id, target, action, detail, .. } => {
            assert_eq!((target.as_str(), action.as_str(), detail.as_str()), ("Repo", "bash", "cargo test"));
            approval_id
        }
        other => panic!("{other:?}"),
    };
    me.send(&ClientMessage::ResolveApproval { approval_id, approved: true, always: false }).await.unwrap();
    assert!(matches!(next(&mut me).await, ServerMessage::ChatEvent { event: ChatEventDto::Tool { status: ToolStatusDto::Completed, .. }, .. }));
    assert!(matches!(next(&mut me).await, ServerMessage::ChatEvent { event: ChatEventDto::Text { text }, .. } if text == "done"));
    // …and the turn ends the usual way.
    match next(&mut me).await {
        ServerMessage::ChatResponse { content, conversation_id, .. } => assert_eq!((content.as_str(), conversation_id.as_deref()), ("done", Some("c1"))),
        other => panic!("{other:?}"),
    }
    assert_eq!(*engine.approvals.lock().unwrap(), [true]);

    // The engine was told the folder, the project's instructions and no session yet.
    {
        let asked = engine.asked.lock().unwrap();
        assert_eq!((asked[0].workdir.as_str(), asked[0].system.as_deref(), asked[0].session_id.clone()), ("/home/me/repo", Some("Use tabs."), None));
    }
    // The conversation is an ordinary one in the project, and its next message goes to the same session — with no
    // project sent: a conversation keeps the one it started in.
    say(&mut me, "now run it", None).await;
    loop {
        match next(&mut me).await {
            ServerMessage::ApprovalRequest { approval_id, .. } => me.send(&ClientMessage::ResolveApproval { approval_id, approved: false, always: false }).await.unwrap(),
            ServerMessage::ChatResponse { .. } => break,
            _ => {}
        }
    }
    assert_eq!(*engine.approvals.lock().unwrap(), [true, false], "a no is a no");
    assert_eq!(engine.asked.lock().unwrap()[1].session_id.as_deref(), Some("ses_1"));

    me.send(&ClientMessage::ListConversations { request_id: 2 }).await.unwrap();
    match next(&mut me).await {
        ServerMessage::ConversationList { conversations, .. } => assert_eq!(conversations.iter().find(|c| c.id == "c1").unwrap().project_id.as_deref(), Some("repo")),
        other => panic!("{other:?}"),
    }
    me.send(&ClientMessage::RequestHistory { request_id: 3, limit: None, conversation_id: Some("c1".into()) }).await.unwrap();
    match next(&mut me).await {
        ServerMessage::History { messages, .. } => assert_eq!(messages.len(), 4),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_stop_finds_the_running_task_and_the_work_so_far_is_kept() {
    let engine = engine();
    let hub = spin_up(Some(engine.clone())).await;
    let mut me = owner(&hub).await;
    save_project(&mut me, "repo", Some("/home/me/repo"), true).await;

    say(&mut me, "HANG", Some("repo")).await;
    assert!(matches!(next(&mut me).await, ServerMessage::ChatEvent { event: ChatEventDto::Tool { .. }, .. }), "the task is running, and its session is known");
    // Another device may stop it: the registry is the hub's, not the connection's.
    let mut elsewhere = ServerConnection::connect(&hub.url, "phone", "Phone", KEY).await.unwrap();
    elsewhere.send(&ClientMessage::CancelTurn { conversation_id: "c1".into() }).await.unwrap();
    match next(&mut me).await {
        ServerMessage::ChatResponse { content, .. } => assert_eq!(content, "stopped half way"),
        other => panic!("{other:?}"),
    }
    assert_eq!(*engine.aborted.lock().unwrap(), [("/home/me/repo".to_string(), "ses_1".to_string())]);

    // Stopping what isn't running is harmless.
    elsewhere.send(&ClientMessage::CancelTurn { conversation_id: "c1".into() }).await.unwrap();
    elsewhere.send(&ClientMessage::Ping { nonce: 7 }).await.unwrap();
    assert!(matches!(elsewhere.recv().await.unwrap(), Some(ServerMessage::Pong { nonce: 7 })));
    assert_eq!(engine.aborted.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_mode_a_device_sets_reaches_the_task_and_a_change_in_the_middle_of_it_does_too() {
    let engine = engine();
    let hub = spin_up(Some(engine.clone())).await;
    let mut me = owner(&hub).await;
    save_project(&mut me, "repo", Some("/home/me/repo"), true).await;

    // A conversation nobody set is manual; one set before its task starts begins in that mode.
    me.send(&ClientMessage::SetCodeMode { conversation_id: "c1".into(), mode: "plan".into() }).await.unwrap();
    say(&mut me, "HANG", Some("repo")).await;
    assert!(matches!(next(&mut me).await, ServerMessage::ChatEvent { .. }), "the task is running");
    let mode = engine.modes.lock().unwrap()[0].clone();
    assert_eq!(*mode.borrow(), CodeMode::Plan);

    // Another device changes it while the task runs: the task's own watch sees it, at once.
    let mut elsewhere = ServerConnection::connect(&hub.url, "phone", "Phone", KEY).await.unwrap();
    elsewhere.send(&ClientMessage::SetCodeMode { conversation_id: "c1".into(), mode: "acceptAll".into() }).await.unwrap();
    elsewhere.send(&ClientMessage::Ping { nonce: 1 }).await.unwrap();
    assert!(matches!(elsewhere.recv().await.unwrap(), Some(ServerMessage::Pong { nonce: 1 })), "messages are handled in order");
    assert_eq!(*mode.borrow(), CodeMode::AcceptAll);

    // A name nobody knows asks the most.
    elsewhere.send(&ClientMessage::SetCodeMode { conversation_id: "c1".into(), mode: "yolo".into() }).await.unwrap();
    elsewhere.send(&ClientMessage::Ping { nonce: 2 }).await.unwrap();
    assert!(matches!(elsewhere.recv().await.unwrap(), Some(ServerMessage::Pong { nonce: 2 })));
    assert_eq!(*mode.borrow(), CodeMode::Manual);

    elsewhere.send(&ClientMessage::CancelTurn { conversation_id: "c1".into() }).await.unwrap();
    assert!(matches!(next(&mut me).await, ServerMessage::ChatResponse { .. }));
}

#[tokio::test]
async fn a_hub_without_an_engine_says_so_and_a_shell_only_project_keeps_the_ordinary_turn() {
    let hub = spin_up(None).await;
    let mut me = owner(&hub).await;
    save_project(&mut me, "repo", Some("/home/me/repo"), true).await;
    say(&mut me, "hello", Some("repo")).await;
    match next(&mut me).await {
        ServerMessage::ChatError { message, .. } => assert!(message.contains("no code engine"), "{message}"),
        other => panic!("{other:?}"),
    }

    // A working folder alone is not code mode: its conversations stay the Warden's own, engine or not.
    let engine = engine();
    let hub = spin_up(Some(engine.clone())).await;
    let mut me = owner(&hub).await;
    save_project(&mut me, "shell-only", Some("/home/me/repo"), false).await;
    say(&mut me, "hello", Some("shell-only")).await;
    match next(&mut me).await {
        ServerMessage::ChatResponse { content, .. } => assert_eq!(content, "ordinary turn"),
        other => panic!("{other:?}"),
    }
    assert!(engine.asked.lock().unwrap().is_empty());
}

struct MemberHost {
    path: std::path::PathBuf,
}

#[async_trait]
impl warden_server::SettingsHost for MemberHost {
    fn config_path(&self) -> std::path::PathBuf {
        self.path.clone()
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        anyhow::bail!("not used by this test")
    }
}

#[tokio::test]
async fn a_member_can_neither_change_the_mode_nor_stop_a_task_that_is_not_theirs() {
    std::env::set_var(warden_core::memory::embed::OFF_SWITCH, "1");
    let dir = std::env::temp_dir().join(format!("warden-server-code-member-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    let mut config = warden_bootstrap::FileConfig::default();
    warden_bootstrap::users::add_user(&mut config, "ana", "Ana", "provisional-1").unwrap();
    warden_bootstrap::save_config(&config_path, &config).unwrap();

    let engine = engine();
    let orchestrator = Orchestrator::new(Arc::new(Plain), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_users_dir(dir.join("users"))
        .with_settings(Arc::new(MemberHost { path: config_path }))
        .with_code_engine(engine.clone());
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    let hub = Hub { url: format!("ws://{addr}") };

    let mut me = owner(&hub).await;
    save_project(&mut me, "repo", Some("/home/me/repo"), true).await;
    say(&mut me, "HANG", Some("repo")).await;
    assert!(matches!(next(&mut me).await, ServerMessage::ChatEvent { .. }), "the task is running");
    let mode = engine.modes.lock().unwrap()[0].clone();
    assert_eq!(*mode.borrow(), CodeMode::Manual);

    // Ana, past her provisional password, names the owner's conversation: she may not loosen it nor stop it.
    let (mut ana, _, _) = ServerConnection::handshake_as_member(&hub.url, "ana-phone", "Ana's phone", "ana", "provisional-1", None, warden_server_protocol::tls::default_client_config()).await.unwrap();
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: "provisional-1".into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(next(&mut ana).await, ServerMessage::PasswordChanged { .. }));
    ana.send(&ClientMessage::SetCodeMode { conversation_id: "c1".into(), mode: "acceptAll".into() }).await.unwrap();
    ana.send(&ClientMessage::CancelTurn { conversation_id: "c1".into() }).await.unwrap();
    ana.send(&ClientMessage::Ping { nonce: 1 }).await.unwrap();
    assert!(matches!(ana.recv().await.unwrap(), Some(ServerMessage::Pong { nonce: 1 })), "messages are handled in order");
    assert_eq!(*mode.borrow(), CodeMode::Manual, "the mode stayed");
    assert!(engine.aborted.lock().unwrap().is_empty(), "the task kept running");

    // The owner still can, so the silence above was the guard and not a broken path.
    me.send(&ClientMessage::CancelTurn { conversation_id: "c1".into() }).await.unwrap();
    assert!(matches!(next(&mut me).await, ServerMessage::ChatResponse { .. }));
    assert_eq!(engine.aborted.lock().unwrap().len(), 1);
}

/// The whole chain with the opencode itself (`cargo test -p warden-server --test code_mode real_ -- --ignored`): a real
/// hub, the opencode started in a real folder with the hub's model as its provider, and a scripted model behind the hub
/// (so it costs nothing). A plain question comes back as the answer; a command goes through the person's approval and
/// then runs in the project's folder. Needs the opencode installed and the network (it fetches its provider package once).
mod real {
    use super::*;
    use warden_core::model::Role;
    use warden_server::SharedOrchestrator;

    struct Model;
    #[async_trait]
    impl ModelProvider for Model {
        async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let last = messages.last().unwrap();
            let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            // The engine's title request carries no tools and no task: anything will do for it.
            if last.role == Role::Tool {
                return Ok(response_stream(Response { content: "the file is there".into(), tool_calls: Vec::new(), usage: None }));
            }
            if last.content.contains("RUN") && names.contains(&"bash") {
                let call = warden_core::model::ToolCall { id: "call_1".into(), name: "bash".into(), arguments: serde_json::json!({"command": "echo made-by-the-engine > out.txt", "description": "Create out.txt"}), thought_signature: None };
                return Ok(response_stream(Response { content: String::new(), tool_calls: vec![call], usage: None }));
            }
            Ok(response_stream(Response { content: "pong from the hub's model".into(), tool_calls: Vec::new(), usage: None }))
        }
    }

    /// The next message that isn't a stray notice. Generous: the first run installs the opencode's provider package.
    async fn wait(conn: &mut ServerConnection) -> ServerMessage {
        loop {
            match tokio::time::timeout(Duration::from_secs(180), conn.recv()).await.expect("no message in time").unwrap().expect("closed") {
                ServerMessage::ConversationsChanged { .. } | ServerMessage::Pong { .. } => continue,
                other => return other,
            }
        }
    }

    #[tokio::test]
    #[ignore = "needs the opencode installed and network access"]
    async fn real_opencode_answers_through_the_hubs_model_and_asks_before_it_runs_a_command() {
        std::env::set_var(warden_core::memory::embed::OFF_SWITCH, "1");
        let dir = std::env::temp_dir().join(format!("warden-real-opencode-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let orchestrator = Orchestrator::new(Arc::new(Model), Arc::new(Vault::new(dir.join("vault"))));
        let shared = SharedOrchestrator::new(orchestrator);
        let models = warden_server::engine_models::EngineModels::start(shared.clone()).await.unwrap();
        let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", shared, dir.join("conversations"), dir.join("devices.json"))
            .await
            .unwrap()
            .with_code_engine(warden_server::code_turns::opencode_engine(&models));
        let addr = server.local_addr().unwrap();
        tokio::spawn(server.serve());
        let mut me = ServerConnection::connect(&format!("ws://{addr}"), "laptop", "Laptop", KEY).await.unwrap();
        save_project(&mut me, "repo", Some(repo.to_str().unwrap()), true).await;

        say(&mut me, "say pong", Some("repo")).await;
        loop {
            match wait(&mut me).await {
                ServerMessage::ChatResponse { content, .. } => {
                    assert!(content.contains("pong from the hub's model"), "{content}");
                    break;
                }
                ServerMessage::ChatError { message, .. } => panic!("{message}"),
                _ => {}
            }
        }

        say(&mut me, "RUN it", None).await;
        let mut asked = false;
        loop {
            match wait(&mut me).await {
                ServerMessage::ApprovalRequest { approval_id, target, action, detail, .. } => {
                    asked = true;
                    assert_eq!((target.as_str(), action.as_str()), ("Repo", "bash"));
                    assert!(detail.contains("made-by-the-engine"), "{detail}");
                    assert!(!repo.join("out.txt").exists(), "nothing ran before the yes");
                    me.send(&ClientMessage::ResolveApproval { approval_id, approved: true, always: false }).await.unwrap();
                }
                ServerMessage::ChatResponse { content, .. } => {
                    assert!(content.contains("the file is there"), "{content}");
                    break;
                }
                ServerMessage::ChatError { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
        assert!(asked, "the command was put to the person");
        assert_eq!(std::fs::read_to_string(repo.join("out.txt")).unwrap().trim(), "made-by-the-engine", "it ran in the project's folder, after the yes");
    }
}
