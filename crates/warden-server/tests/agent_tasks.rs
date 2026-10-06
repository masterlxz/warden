//! P123 on the hub: `ListAgentTasks` shows the tasks agents delegated to each other, read from the log the orchestrator writes.

use std::path::PathBuf;
use std::sync::Arc;

use warden_bootstrap::agent_tasks::FileTaskRecorder;
use warden_core::jobs::{TaskOutcome, TaskRecorder, TaskSpec};
use warden_core::memory::Vault;
use warden_core::model::Usage;
use warden_core::orchestrator::Orchestrator;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage};

struct NoModel;

#[async_trait::async_trait]
impl warden_core::model::ModelProvider for NoModel {
    async fn chat_stream(&self, _messages: Vec<warden_core::model::Message>, _tools: Vec<warden_core::tool::ToolSpec>) -> anyhow::Result<warden_core::model::ChatStream> {
        anyhow::bail!("these tests never run a turn")
    }
}

async fn spin_up(log: Option<PathBuf>) -> String {
    let dir = std::env::temp_dir().join(format!("warden-server-agent-tasks-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let orchestrator = Orchestrator::new(Arc::new(NoModel), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_agent_tasks(log);
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    format!("ws://{addr}")
}

fn spec(assignee: &str, group: &str, model: Option<&str>) -> TaskSpec {
    TaskSpec { group: group.into(), owner: Some("chief".into()), assignee: assignee.into(), objective: format!("do the {assignee} part"), model: model.map(str::to_string), channel: "desktop".into(), parent: None }
}

async fn list(url: &str) -> Vec<warden_server_protocol::protocol::AgentTaskDto> {
    let mut conn = ServerConnection::connect(url, "web-1", "Browser", "test-key").await.unwrap();
    conn.send(&ClientMessage::ListAgentTasks { request_id: 7 }).await.unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            ServerMessage::AgentTaskList { request_id, tasks } => {
                assert_eq!(request_id, 7);
                return tasks;
            }
            _ => continue,
        }
    }
}

#[tokio::test]
async fn the_tasks_in_the_log_are_listed_newest_first_with_their_state_model_and_tokens() {
    let log = std::env::temp_dir().join(format!("warden-agent-task-log-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())).join("agent_tasks.jsonl");
    let recorder = FileTaskRecorder::new(&log);
    let done = recorder.created(&spec("backend", "turn-1", Some("strong")));
    recorder.running(&done);
    recorder.finished(&done, TaskOutcome::Done { result: "an API".into(), usage: Some(Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }) });
    let running = recorder.created(&spec("tests", "turn-1", None));
    recorder.running(&running);
    recorder.created(&spec("docs", "turn-1", Some("fast")));

    let url = spin_up(Some(log)).await;
    let tasks = list(&url).await;

    assert_eq!(tasks.iter().map(|t| (t.assignee.as_str(), t.state.as_str())).collect::<Vec<_>>(), [("docs", "pending"), ("tests", "running"), ("backend", "done")]);
    let backend = tasks.iter().find(|t| t.assignee == "backend").unwrap();
    assert_eq!((backend.model.as_deref(), backend.owner.as_deref(), backend.group.as_str()), (Some("strong"), Some("chief"), "turn-1"));
    assert_eq!((backend.result.as_deref(), backend.prompt_tokens, backend.completion_tokens, backend.total_tokens), (Some("an API"), Some(10), Some(5), Some(15)));
    assert_eq!(backend.objective, "do the backend part");
    assert!(backend.started_at_ms.is_some() && backend.finished_at_ms.is_some());
    assert!(tasks.iter().find(|t| t.assignee == "docs").unwrap().started_at_ms.is_none());
}

#[tokio::test]
async fn a_subtask_lists_its_parent_and_a_task_waiting_for_it_is_waiting() {
    let log = std::env::temp_dir().join(format!("warden-agent-task-nest-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())).join("agent_tasks.jsonl");
    let recorder = FileTaskRecorder::new(&log);
    let manager = recorder.created(&spec("manager", "turn-1", None));
    recorder.running(&manager);
    let helper = recorder.created(&TaskSpec { parent: Some(manager.clone()), owner: Some("manager".into()), ..spec("helper", "turn-1", None) });
    recorder.running(&helper);
    recorder.waiting(&manager);

    let tasks = list(&spin_up(Some(log)).await).await;

    let manager = tasks.iter().find(|t| t.assignee == "manager").unwrap();
    let helper = tasks.iter().find(|t| t.assignee == "helper").unwrap();
    assert_eq!((manager.state.as_str(), manager.parent_id.as_deref()), ("waiting", None));
    assert_eq!((helper.state.as_str(), helper.parent_id.as_deref(), helper.group.as_str()), ("running", Some(manager.id.as_str()), "turn-1"));
}

/// Sends `message` and returns the first answer that is a task list or a task error.
async fn ask(conn: &mut ServerConnection, message: ClientMessage) -> ServerMessage {
    conn.send(&message).await.unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            reply @ (ServerMessage::AgentTaskList { .. } | ServerMessage::TaskError { .. }) => return reply,
            _ => continue,
        }
    }
}

fn control(request_id: u64, key: &str, task_id: &str, action: &str) -> ClientMessage {
    ClientMessage::ControlAgentTask { request_id, pairing_key: key.into(), task_id: task_id.into(), action: action.into() }
}

#[tokio::test]
async fn the_owner_pauses_resumes_and_stops_a_task_running_on_the_hub_and_is_told_why_when_it_cannot() {
    use warden_core::jobs::{JobBoard, TaskContext};

    let log = std::env::temp_dir().join(format!("warden-agent-task-control-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())).join("agent_tasks.jsonl");
    let recorder = Arc::new(FileTaskRecorder::new(&log));
    // One task really running in this process (what the hub's orchestrator would have started), and one only the log knows.
    let board = JobBoard::recording(2, recorder.clone(), TaskContext { group: "turn-1".into(), owner: Some("chief".into()), channel: "desktop".into(), parent: None, depth: 0 });
    board.spawn_task("backend: build".into(), warden_core::jobs::TaskDraft { assignee: "backend".into(), objective: "build".into(), model: None }, std::future::pending());
    let elsewhere = recorder.created(&spec("elsewhere", "turn-0", None));
    recorder.running(&elsewhere);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let url = spin_up(Some(log)).await;
    let mut conn = ServerConnection::connect(&url, "web-1", "Browser", "test-key").await.unwrap();
    let tasks = list(&url).await;
    let mine = tasks.iter().find(|t| t.assignee == "backend").unwrap();
    assert!(mine.controllable && !tasks.iter().find(|t| t.assignee == "elsewhere").unwrap().controllable);
    let id = mine.id.clone();
    let state_of = |reply: ServerMessage, assignee: &str| match reply {
        ServerMessage::AgentTaskList { tasks, .. } => tasks.into_iter().find(|t| t.assignee == assignee).map(|t| (t.state, t.error, t.controllable)).unwrap(),
        other => panic!("expected the list, got {other:?}"),
    };
    let refusal = |reply: ServerMessage| match reply {
        ServerMessage::TaskError { message, auth_rejected, .. } => (message, auth_rejected),
        other => panic!("expected an error, got {other:?}"),
    };

    // A wrong key changes nothing.
    assert_eq!(refusal(ask(&mut conn, control(1, "wrong", &id, "pause")).await), ("wrong pairing key".to_string(), true));

    assert_eq!(state_of(ask(&mut conn, control(2, "test-key", &id, "pause")).await, "backend"), ("paused".to_string(), None, true));
    assert!(refusal(ask(&mut conn, control(3, "test-key", &id, "pause")).await).0.contains("already paused"));
    assert_eq!(state_of(ask(&mut conn, control(4, "test-key", &id, "resume")).await, "backend").0, "running");
    assert!(refusal(ask(&mut conn, control(5, "test-key", &id, "resume")).await).0.contains("isn't paused"));
    assert!(refusal(ask(&mut conn, control(6, "test-key", &elsewhere, "cancel")).await).0.contains("isn't running on this machine"));
    assert!(refusal(ask(&mut conn, control(7, "test-key", &id, "explode")).await).0.contains("unknown action"));
    assert!(refusal(ask(&mut conn, control(8, "test-key", "at-nope", "cancel")).await).0.contains("no task"));

    let (state, error, controllable) = state_of(ask(&mut conn, control(9, "test-key", &id, "cancel")).await, "backend");
    assert_eq!((state.as_str(), error.as_deref(), controllable), ("cancelled", Some("stopped by a person"), false));
    assert!(refusal(ask(&mut conn, control(10, "test-key", &id, "pause")).await).0.contains("already ended"));
}

#[tokio::test]
async fn a_hub_with_no_log_or_no_tasks_lists_nothing() {
    assert!(list(&spin_up(None).await).await.is_empty());
    let empty = std::env::temp_dir().join("warden-no-agent-task-log-here").join("agent_tasks.jsonl");
    assert!(list(&spin_up(Some(empty)).await).await.is_empty());
}
