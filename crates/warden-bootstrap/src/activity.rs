//! The feed of activity (P121): who did what, newest first, so a person can follow a set of agents without opening each conversation.
//!
//! Nothing here is written down on purpose. The feed is read out of what the hub already keeps, when a screen asks:
//! 1. the tasks agents delegated to each other (`agent_tasks`): who delegated, who started, who finished and how;
//! 2. the notes agents leave each other (`message_agent`, the conversations "A → B"): the note and the answer;
//! 3. the messages an agent started to the person (`message_user`, the agent's channel): an assistant message that does not answer a
//!    message of the person.
//!
//! So it also covers what happened before it existed, and no part of the orchestrator has to know about it. What is not there is
//! what nothing records: an agent created or removed by another, and the runs of scheduled tasks and webhooks.

use std::path::Path;

use crate::agent_tasks::{read_agent_tasks, AgentTask, TaskState};
use crate::message_agent::CHANNEL_PREFIX;
use crate::{load_conversation, ChatRole, Conversation};

/// The most events a read hands back (the newest ones).
pub const MAX_EVENTS: usize = 150;
/// The most characters of a task's objective, result or error, or of a message, kept in an event.
const MAX_TEXT_CHARS: usize = 200;
/// Prefix of the id of the conversation between two agents (`message_agent::thread_id`).
const THREAD_PREFIX: &str = "agents-";

/// One thing that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEvent {
    /// Stable for the same happening, so a screen can tell an event it already shows.
    pub id: String,
    pub at_ms: u64,
    /// `delegated`, `started`, `done`, `failed`, `cancelled`, `note`, `reply` or `messaged_user`.
    pub kind: &'static str,
    /// The agent that did it.
    pub actor: String,
    /// The agent it was done to, when there is one.
    pub target: Option<String>,
    /// What was asked, answered or said, cut short.
    pub text: String,
    /// The delegated task it is about (`kind` is one of the task ones).
    pub task_id: Option<String>,
    /// The conversation it is in (`note`, `reply`, `messaged_user`), to open it.
    pub conversation_id: Option<String>,
}

fn clip(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_TEXT_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(MAX_TEXT_CHARS).collect();
    cut.push('…');
    cut
}

/// What a person calls the agent that started a task nobody delegated: the assistant they talk to without choosing an agent.
const NO_AGENT: &str = "";

/// The events of delegated tasks: the delegation, the start, and the end of each.
pub fn task_events(tasks: &[AgentTask]) -> Vec<ActivityEvent> {
    let mut events = Vec::new();
    for task in tasks {
        let task_id = Some(task.id.clone());
        events.push(ActivityEvent {
            id: format!("{}-delegated", task.id),
            at_ms: task.created_at_ms,
            kind: "delegated",
            actor: task.owner.clone().unwrap_or_else(|| NO_AGENT.to_string()),
            target: Some(task.assignee.clone()),
            text: clip(&task.objective),
            task_id: task_id.clone(),
            conversation_id: None,
        });
        if let Some(at) = task.started_at_ms {
            events.push(ActivityEvent {
                id: format!("{}-started", task.id),
                at_ms: at,
                kind: "started",
                actor: task.assignee.clone(),
                target: None,
                text: clip(&task.objective),
                task_id: task_id.clone(),
                conversation_id: None,
            });
        }
        let (kind, text) = match task.state {
            TaskState::Done => ("done", task.result.as_deref().unwrap_or_default()),
            TaskState::Failed => ("failed", task.error.as_deref().unwrap_or_default()),
            TaskState::Cancelled => ("cancelled", task.error.as_deref().unwrap_or_default()),
            _ => continue,
        };
        events.push(ActivityEvent {
            id: format!("{}-{kind}", task.id),
            at_ms: task.finished_at_ms.unwrap_or(task.created_at_ms),
            kind,
            actor: task.assignee.clone(),
            target: None,
            text: clip(text),
            task_id,
            conversation_id: None,
        });
    }
    events
}

/// The events of one conversation between two agents ("A → B"): a note A left, and what B answered.
pub fn thread_events(conversation: &Conversation) -> Vec<ActivityEvent> {
    let Some((from, to)) = conversation.title.split_once(" → ") else { return Vec::new() };
    let (from, to) = (from.trim(), to.trim());
    conversation
        .messages
        .iter()
        .map(|message| {
            let (kind, actor, target) = match message.role {
                ChatRole::User => ("note", from, to),
                ChatRole::Assistant => ("reply", to, from),
            };
            ActivityEvent {
                id: format!("{}-{}", conversation.id, message.id),
                at_ms: message.created_at.max(0) as u64,
                kind,
                actor: actor.to_string(),
                target: Some(target.to_string()),
                text: clip(&message.content),
                task_id: None,
                conversation_id: Some(conversation.id.clone()),
            }
        })
        .collect()
}

/// The events of an agent's channel: the messages it started. One that answers the person (it follows a message of theirs) is not an event.
pub fn channel_events(conversation: &Conversation) -> Vec<ActivityEvent> {
    let mut events = Vec::new();
    let mut previous = None;
    for message in &conversation.messages {
        if message.role == ChatRole::Assistant && previous != Some(ChatRole::User) {
            events.push(ActivityEvent {
                id: format!("{}-{}", conversation.id, message.id),
                at_ms: message.created_at.max(0) as u64,
                kind: "messaged_user",
                actor: conversation.title.clone(),
                target: None,
                text: clip(&message.content),
                task_id: None,
                conversation_id: Some(conversation.id.clone()),
            });
        }
        previous = Some(message.role);
    }
    events
}

/// How far along a happening is, to order events that carry the same time.
fn step(kind: &str) -> u8 {
    match kind {
        "done" | "failed" | "cancelled" => 2,
        "started" | "reply" => 1,
        _ => 0,
    }
}

/// The feed of `tasks` and `conversations` (any others are ignored): newest first, at most `limit`.
pub fn activity_feed(tasks: &[AgentTask], conversations: &[Conversation], limit: usize) -> Vec<ActivityEvent> {
    let mut events = task_events(tasks);
    for conversation in conversations {
        if conversation.id.starts_with(THREAD_PREFIX) {
            events.extend(thread_events(conversation));
        } else if conversation.id.starts_with(CHANNEL_PREFIX) {
            events.extend(channel_events(conversation));
        }
    }
    // Newest first. Within the same millisecond the later step comes first (an end before a start before the delegation, an answer before
    // the note), and the id settles the rest so the order is the same every time it is asked.
    events.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| step(b.kind).cmp(&step(a.kind))).then_with(|| a.id.cmp(&b.id)));
    events.truncate(limit);
    events
}

/// The feed read from the task log at `tasks_log` and the conversations in each of `conversations_dirs` (the owner keeps those of a run nobody
/// watched apart from the others). Only the files of the conversations between agents and of agents' channels are opened, so a directory with
/// many loose conversations costs nothing; one that cannot be read (a missing file, a locked directory) is skipped.
pub fn read_activity(tasks_log: Option<&Path>, conversations_dirs: &[&Path], limit: usize) -> Vec<ActivityEvent> {
    let tasks = tasks_log.map(read_agent_tasks).unwrap_or_default();
    let mut conversations = Vec::new();
    for dir in conversations_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else { continue };
            if !(id.starts_with(THREAD_PREFIX) || id.starts_with(CHANNEL_PREFIX)) {
                continue;
            }
            if let Ok(Some(conversation)) = load_conversation(dir, id) {
                conversations.push(conversation);
            }
        }
    }
    activity_feed(&tasks, &conversations, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConversationMessage;

    fn task(id: &str, owner: Option<&str>, assignee: &str, state: TaskState) -> AgentTask {
        AgentTask {
            id: id.into(),
            group: "g".into(),
            owner: owner.map(Into::into),
            assignee: assignee.into(),
            parent_id: None,
            objective: "write the API".into(),
            model: None,
            channel: "web".into(),
            state,
            result: None,
            error: None,
            usage: None,
            created_at_ms: 1_000,
            started_at_ms: None,
            finished_at_ms: None,
        }
    }

    fn message(id: &str, role: ChatRole, content: &str, at: i64) -> ConversationMessage {
        ConversationMessage {
            id: id.into(),
            role,
            content: content.into(),
            created_at: at,
            usage: None,
            attachments: Vec::new(),
            generated_files: Vec::new(),
            tools_used: Vec::new(),
        }
    }

    fn conversation(id: &str, title: &str, messages: Vec<ConversationMessage>) -> Conversation {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "messages": messages, "createdAt": 1, "updatedAt": 1
        }))
        .unwrap()
    }

    #[test]
    fn a_finished_task_tells_who_delegated_who_started_and_how_it_ended() {
        let mut done = task("t1", Some("manager"), "backend", TaskState::Done);
        done.started_at_ms = Some(2_000);
        done.finished_at_ms = Some(3_000);
        done.result = Some("the API is   ready\nwith tests".into());
        let events = task_events(&[done]);
        let summary: Vec<_> = events.iter().map(|e| (e.kind, e.actor.as_str(), e.target.as_deref(), e.at_ms)).collect();
        assert_eq!(summary, [("delegated", "manager", Some("backend"), 1_000), ("started", "backend", None, 2_000), ("done", "backend", None, 3_000)]);
        assert_eq!(events[2].text, "the API is ready with tests", "white space is flattened");
        assert!(events.iter().all(|e| e.task_id.as_deref() == Some("t1")));
    }

    #[test]
    fn a_task_that_failed_or_was_cancelled_says_why_and_a_running_one_has_no_end() {
        let mut failed = task("t1", Some("m"), "a", TaskState::Failed);
        failed.error = Some("the model refused".into());
        failed.finished_at_ms = Some(5_000);
        let mut cancelled = task("t2", Some("m"), "b", TaskState::Cancelled);
        cancelled.error = Some("stopped by a person".into());
        let running = task("t3", Some("m"), "c", TaskState::Running);
        let ends: Vec<_> = task_events(&[failed, cancelled, running]).into_iter().filter(|e| matches!(e.kind, "failed" | "cancelled")).map(|e| (e.kind, e.text)).collect();
        assert_eq!(ends, [("failed", "the model refused".to_string()), ("cancelled", "stopped by a person".to_string())]);
        assert_eq!(task_events(&[task("t3", None, "c", TaskState::Running)]).len(), 1, "only the delegation");
    }

    #[test]
    fn a_task_nobody_delegated_has_no_actor_name() {
        assert_eq!(task_events(&[task("t1", None, "writer", TaskState::Pending)])[0].actor, "");
    }

    #[test]
    fn a_note_and_its_answer_are_told_from_the_title_of_the_conversation() {
        let thread = conversation(
            "agents-00ff",
            "ana → bia",
            vec![message("m1", ChatRole::User, "can you review this?", 10), message("m2", ChatRole::Assistant, "looks fine", 20)],
        );
        let events = thread_events(&thread);
        let summary: Vec<_> = events.iter().map(|e| (e.kind, e.actor.as_str(), e.target.as_deref())).collect();
        assert_eq!(summary, [("note", "ana", Some("bia")), ("reply", "bia", Some("ana"))]);
        assert_eq!(events[0].conversation_id.as_deref(), Some("agents-00ff"));
        assert!(thread_events(&conversation("agents-1", "no arrow", vec![message("m", ChatRole::User, "x", 1)])).is_empty());
    }

    #[test]
    fn only_a_message_the_agent_started_counts_in_its_channel() {
        let channel = conversation(
            "channel-00ff",
            "pirate",
            vec![
                message("m1", ChatRole::Assistant, "the disk is full", 10),
                message("m2", ChatRole::User, "how full?", 20),
                message("m3", ChatRole::Assistant, "98%", 30),
                message("m4", ChatRole::Assistant, "and rising", 40),
            ],
        );
        let events = channel_events(&channel);
        let said: Vec<_> = events.iter().map(|e| (e.actor.as_str(), e.text.as_str())).collect();
        assert_eq!(said, [("pirate", "the disk is full"), ("pirate", "and rising")], "the answer to the person is not the agent speaking first");
    }

    #[test]
    fn the_feed_is_newest_first_limited_and_ignores_loose_conversations() {
        let mut done = task("t1", Some("m"), "a", TaskState::Done);
        done.finished_at_ms = Some(500);
        let thread = conversation("agents-1", "x → y", vec![message("m1", ChatRole::User, "hi", 900)]);
        let loose = conversation("some-chat", "x → y", vec![message("m1", ChatRole::User, "hi", 9_999)]);
        let feed = activity_feed(&[done], &[thread, loose], 10);
        let order: Vec<_> = feed.iter().map(|e| e.kind).collect();
        assert_eq!(order, ["delegated", "note", "done"], "at 1000, 900 and 500; the loose conversation (9999) is left out");
    }

    #[test]
    fn the_limit_keeps_the_newest_and_a_long_text_is_cut() {
        let mut tasks = Vec::new();
        for n in 0..5 {
            let mut t = task(&format!("t{n}"), Some("m"), "a", TaskState::Pending);
            t.created_at_ms = 100 * (n + 1);
            tasks.push(t);
        }
        let feed = activity_feed(&tasks, &[], 2);
        assert_eq!(feed.iter().map(|e| e.at_ms).collect::<Vec<_>>(), [500, 400]);
        let mut long = task("t", Some("m"), "a", TaskState::Pending);
        long.objective = "x".repeat(500);
        let text = task_events(&[long])[0].text.clone();
        assert_eq!(text.chars().count(), MAX_TEXT_CHARS + 1);
        assert!(text.ends_with('…'));
    }
}
