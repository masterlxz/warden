//! P121 on the hub: `ListActivity` shows who delegated, started and finished what, the notes agents left each other and the messages they
//! started to the person, read from the files the hub already keeps.

use std::path::PathBuf;
use std::sync::Arc;

use warden_bootstrap::agent_changes::{self, AgentChange};
use warden_bootstrap::agent_tasks::FileTaskRecorder;
use warden_bootstrap::message_agent::{channel_id, thread_id, thread_title};
use warden_bootstrap::{append_messages, AppendOptions, ChatRole, ConversationMessage};
use warden_core::jobs::{TaskOutcome, TaskRecorder, TaskSpec};
use warden_core::memory::Vault;
use warden_core::orchestrator::Orchestrator;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};

struct NoModel;

#[async_trait::async_trait]
impl warden_core::model::ModelProvider for NoModel {
    async fn chat_stream(&self, _messages: Vec<warden_core::model::Message>, _tools: Vec<warden_core::tool::ToolSpec>) -> anyhow::Result<warden_core::model::ChatStream> {
        anyhow::bail!("these tests never run a turn")
    }
}

fn unique_dir(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("warden-server-{what}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The settings file the hub was started with: the log of agents created and removed sits next to it.
struct Settings(PathBuf);

#[async_trait::async_trait]
impl SettingsHost for Settings {
    fn config_path(&self) -> PathBuf {
        self.0.clone()
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        anyhow::bail!("these tests never rebuild")
    }
}

async fn spin_up(dir: &std::path::Path, log: Option<PathBuf>) -> String {
    let orchestrator = Orchestrator::new(Arc::new(NoModel), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_agent_tasks(log)
        .with_settings(Arc::new(Settings(dir.join("config.toml"))));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    format!("ws://{addr}")
}

fn message(id: &str, role: ChatRole, content: &str, at: i64) -> ConversationMessage {
    ConversationMessage { id: id.into(), role, content: content.into(), created_at: at, usage: None, answered_by: None, attachments: Vec::new(), generated_files: Vec::new(), tools_used: Vec::new() }
}

/// Writes a conversation the way the hub would, in the owner's folder.
fn seed(dir: &std::path::Path, id: &str, title: &str, messages: Vec<ConversationMessage>) {
    let owner = dir.join("conversations").join("root");
    append_messages(&owner, id, AppendOptions { title_seed: title, create: true, ..Default::default() }, messages).unwrap();
}

async fn list(url: &str) -> Vec<warden_server_protocol::protocol::ActivityEventDto> {
    let mut conn = ServerConnection::connect(url, "web-1", "Browser", "test-key").await.unwrap();
    conn.send(&ClientMessage::ListActivity { request_id: 7 }).await.unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            ServerMessage::ActivityList { request_id, events } => {
                assert_eq!(request_id, 7);
                return events;
            }
            _ => continue,
        }
    }
}

#[tokio::test]
async fn the_feed_joins_tasks_notes_and_messages_the_agent_started_newest_first() {
    let dir = unique_dir("activity");
    let log = dir.join("agent_tasks.jsonl");
    let recorder = FileTaskRecorder::new(&log);
    let spec = TaskSpec { group: "turn-1".into(), owner: Some("manager".into()), assignee: "backend".into(), objective: "build the API".into(), model: None, channel: "desktop".into(), parent: None };
    let id = recorder.created(&spec);
    recorder.running(&id);
    recorder.finished(&id, TaskOutcome::Done { result: "an API".into(), usage: None });

    // Far in the future, so these are the newest whatever the clock says.
    let later = 9_000_000_000_000;
    seed(&dir, &thread_id("ana", "bia"), &thread_title("ana", "bia"), vec![message("m1", ChatRole::User, "review this?", later), message("m2", ChatRole::Assistant, "looks fine", later + 1)]);
    seed(
        &dir,
        &channel_id("pirate"),
        "pirate",
        vec![message("m1", ChatRole::Assistant, "the disk is full", later + 2), message("m2", ChatRole::User, "how full?", later + 3), message("m3", ChatRole::Assistant, "98%", later + 4)],
    );
    // A loose conversation is not activity.
    seed(&dir, "loose-chat", "ana → bia", vec![message("m1", ChatRole::User, "hello", later + 5)]);

    let events = list(&spin_up(&dir, Some(log)).await).await;

    let summary: Vec<_> = events.iter().map(|e| (e.kind.as_str(), e.actor.as_str())).collect();
    assert_eq!(
        summary,
        [("messaged_user", "pirate"), ("reply", "bia"), ("note", "ana"), ("done", "backend"), ("started", "backend"), ("delegated", "manager")],
        "newest first; the answer to the person and the loose conversation are not there"
    );
    let note = events.iter().find(|e| e.kind == "note").unwrap();
    assert_eq!((note.target.as_deref(), note.text.as_str(), note.conversation_id.as_deref()), (Some("bia"), "review this?", Some(thread_id("ana", "bia").as_str())));
    let done = events.iter().find(|e| e.kind == "done").unwrap();
    assert_eq!((done.text.as_str(), done.task_id.as_deref()), ("an API", Some(id.as_str())));
}

#[tokio::test]
async fn a_scheduled_run_and_an_agent_another_one_created_are_in_the_feed() {
    let dir = unique_dir("activity-runs");
    let later = 9_000_000_000_000;
    seed(
        &dir,
        "task-daily",
        "Tarefa: daily",
        vec![message("m1", ChatRole::User, "[Scheduled task 'daily', today]\n\nsum up", later), message("m2", ChatRole::Assistant, "all quiet", later + 1)],
    );
    let change = AgentChange { at_ms: later as u64 + 2, kind: "created".into(), actor: "chief".into(), agent: "poet".into(), detail: "Poet".into() };
    agent_changes::record(&agent_changes::beside(&dir.join("config.toml")), &change);

    let events = list(&spin_up(&dir, None).await).await;

    let summary: Vec<_> = events.iter().map(|e| (e.kind.as_str(), e.actor.as_str(), e.target.as_deref())).collect();
    assert_eq!(summary, [("created_agent", "chief", Some("poet")), ("scheduled_ran", "", Some("daily"))]);
    assert_eq!(events[1].conversation_id.as_deref(), Some("task-daily"));
}

#[tokio::test]
async fn a_hub_with_nothing_recorded_has_an_empty_feed() {
    let dir = unique_dir("activity-empty");
    assert!(list(&spin_up(&dir, None).await).await.is_empty());
}
