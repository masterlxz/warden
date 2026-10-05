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
    TaskSpec { group: group.into(), owner: Some("chief".into()), assignee: assignee.into(), objective: format!("do the {assignee} part"), model: model.map(str::to_string), channel: "desktop".into() }
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
async fn a_hub_with_no_log_or_no_tasks_lists_nothing() {
    assert!(list(&spin_up(None).await).await.is_empty());
    let empty = std::env::temp_dir().join("warden-no-agent-task-log-here").join("agent_tasks.jsonl");
    assert!(list(&spin_up(Some(empty)).await).await.is_empty());
}
