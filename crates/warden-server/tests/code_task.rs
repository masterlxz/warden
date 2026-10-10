//! `code_task` (P89, P124) on a real hub: a model calls the tool in an ordinary chat turn, the engine's ask goes to the
//! person as an approval card and the answer goes back to the engine, and the model gets what the engine did. The engine is
//! a script (the opencode itself is covered in `warden-core`), and the tool is registered the way the hub's `main` does it
//! (`register_code_task`), with the same slot the engine is put in afterwards.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use warden_core::code_engine::{CodeEngine, CodeEvent, CodeMode, TurnOutcome, TurnRequest};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::code_task::CodeEngineSlot;
use warden_core::tool::{ApprovalRequest, Approver, ToolSpec};
use warden_server::code_turns::register_code_task;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage};
use warden_server_protocol::protocol::ProjectDto;

const KEY: &str = "test-key";

/// Asks for `code_task` on the first call and, once the tool answered, says what it answered.
struct Manager {
    offered: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ModelProvider for Manager {
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        *self.offered.lock().unwrap() = tools.iter().map(|t| t.name.clone()).collect();
        let last = messages.last().unwrap();
        if last.role == Role::Tool {
            return Ok(response_stream(Response { content: format!("tool said: {}", last.content), tool_calls: Vec::new(), usage: None }));
        }
        let call = ToolCall { id: "call-1".into(), name: "code_task".into(), arguments: json!({ "project": "repo", "task": "add a test" }), thought_signature: None };
        Ok(response_stream(Response { content: String::new(), tool_calls: vec![call], usage: None }))
    }
}

/// A task that wants one yes (to run `cargo test`) and says whether it got it.
#[derive(Default)]
struct Scripted {
    asked: Mutex<Vec<(String, CodeMode)>>,
    approvals: Mutex<Vec<bool>>,
}

#[async_trait]
impl CodeEngine for Scripted {
    async fn run_turn(&self, request: TurnRequest, approver: Option<Arc<dyn Approver>>, _: &mut (dyn FnMut(CodeEvent) + Send)) -> anyhow::Result<TurnOutcome> {
        self.asked.lock().unwrap().push((request.workdir.clone(), *request.mode.borrow()));
        let allowed = match approver {
            Some(approver) => approver.approve(ApprovalRequest::new("Repo", "bash", "cargo test")).await,
            None => false,
        };
        self.approvals.lock().unwrap().push(allowed);
        let text = if allowed { "tests added" } else { "the person refused" };
        Ok(TurnOutcome { session_id: "ses_9".into(), text: text.into(), tools_used: vec!["bash".into()] })
    }

    async fn abort(&self, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

struct Hub {
    url: String,
    offered: Arc<Mutex<Vec<String>>>,
}

/// A hub whose engine is `engine` — or, with `None`, one whose slot is never filled.
async fn spin_up(engine: Option<Arc<Scripted>>) -> Hub {
    std::env::set_var(warden_core::memory::embed::OFF_SWITCH, "1");
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("warden-server-code-task-{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let offered = Arc::new(Mutex::new(Vec::new()));
    let mut orchestrator = Orchestrator::new(Arc::new(Manager { offered: offered.clone() }), Arc::new(Vault::new(dir.join("vault"))));
    let slot: CodeEngineSlot = Arc::new(OnceLock::new());
    register_code_task(&mut orchestrator, &slot);
    if let Some(engine) = engine {
        let engine: Arc<dyn CodeEngine> = engine;
        slot.set(engine).ok();
    }
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json")).await.unwrap().with_users_dir(dir.join("users"));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}"), offered }
}

async fn owner(hub: &Hub) -> ServerConnection {
    ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap()
}

async fn next(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), conn.recv()).await.expect("no message").unwrap().expect("connection closed") {
            ServerMessage::ConversationsChanged { .. } | ServerMessage::Pong { .. } | ServerMessage::ChatEvent { .. } => continue,
            msg => return msg,
        }
    }
}

async fn save_code_project(conn: &mut ServerConnection) {
    let project = ProjectDto { id: "repo".into(), name: "Repo".into(), description: "The main repo".into(), instructions: "Use tabs.".into(), workdir: Some("/home/me/repo".into()), code: true };
    conn.send(&ClientMessage::SaveProject { request_id: 1, project, overwrite: false }).await.unwrap();
    assert!(matches!(next(conn).await, ServerMessage::ProjectOk { .. }));
}

async fn ask_for_the_task(conn: &mut ServerConnection) {
    conn.send(&ClientMessage::Chat { message: "get the repo tested".into(), conversation_id: Some("c1".into()), attachments: Vec::new(), agent_id: None, project_id: None, workdir: None, thread_of: None }).await.unwrap();
}

#[tokio::test]
async fn the_engines_ask_reaches_the_person_and_the_model_gets_what_the_engine_did() {
    let engine = Arc::new(Scripted::default());
    let hub = spin_up(Some(engine.clone())).await;
    let mut me = owner(&hub).await;
    save_code_project(&mut me).await;

    ask_for_the_task(&mut me).await;
    let approval_id = match next(&mut me).await {
        ServerMessage::ApprovalRequest { approval_id, target, action, detail, .. } => {
            assert_eq!((target.as_str(), action.as_str(), detail.as_str()), ("Repo", "bash", "cargo test"));
            approval_id
        }
        other => panic!("{other:?}"),
    };
    me.send(&ClientMessage::ResolveApproval { approval_id, approved: true, always: false }).await.unwrap();
    match next(&mut me).await {
        ServerMessage::ChatResponse { content, .. } => {
            assert!(content.starts_with("tool said: "), "{content}");
            assert!(content.contains("tests added") && content.contains("ses_9"), "{content}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(*engine.asked.lock().unwrap(), [("/home/me/repo".to_string(), CodeMode::Manual)]);
    assert_eq!(*engine.approvals.lock().unwrap(), [true]);
    assert!(hub.offered.lock().unwrap().iter().any(|t| t == "code_task"), "the model was offered the tool");
}

#[tokio::test]
async fn a_no_from_the_person_reaches_the_engine() {
    let engine = Arc::new(Scripted::default());
    let hub = spin_up(Some(engine.clone())).await;
    let mut me = owner(&hub).await;
    save_code_project(&mut me).await;

    ask_for_the_task(&mut me).await;
    let approval_id = match next(&mut me).await {
        ServerMessage::ApprovalRequest { approval_id, .. } => approval_id,
        other => panic!("{other:?}"),
    };
    me.send(&ClientMessage::ResolveApproval { approval_id, approved: false, always: false }).await.unwrap();
    match next(&mut me).await {
        ServerMessage::ChatResponse { content, .. } => assert!(content.contains("the person refused"), "{content}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(*engine.approvals.lock().unwrap(), [false]);
}

#[tokio::test]
async fn a_hub_whose_engine_never_started_tells_the_model_instead_of_hanging() {
    let hub = spin_up(None).await;
    let mut me = owner(&hub).await;
    save_code_project(&mut me).await;

    ask_for_the_task(&mut me).await;
    match next(&mut me).await {
        ServerMessage::ChatResponse { content, .. } => assert!(content.contains("isn't available"), "{content}"),
        other => panic!("{other:?}"),
    }
}
