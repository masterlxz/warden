//! The work agents delegated to each other (P123), for the screens: `ListAgentTasks` reads the log the hub's orchestrator writes
//! (`warden_bootstrap::agent_tasks`). Read only, and the owner's: tasks hold what agents were asked and what they answered, which a
//! member has no business reading in the owner's log, so a member gets an empty list.

use std::path::Path;

use warden_bootstrap::agent_tasks::{control_agent_task, read_agent_tasks, AgentTask, TaskControl};
use warden_server_protocol::protocol::AgentTaskDto;
use warden_server_protocol::ServerMessage;

use crate::settings::{keys_match, WRONG_KEY_DELAY};

fn to_dto(task: AgentTask) -> AgentTaskDto {
    let controllable = warden_core::jobs::task_controls().is_controllable(&task.id);
    let pausable = warden_core::jobs::task_controls().is_pausable(&task.id);
    AgentTaskDto {
        controllable,
        pausable,
        id: task.id,
        group: task.group,
        owner: task.owner,
        assignee: task.assignee,
        parent_id: task.parent_id,
        objective: task.objective,
        model: task.model,
        channel: task.channel,
        state: task.state.as_str().to_string(),
        result: task.result,
        error: task.error,
        prompt_tokens: task.usage.map(|u| u.prompt_tokens),
        completion_tokens: task.usage.map(|u| u.completion_tokens),
        total_tokens: task.usage.map(|u| u.total_tokens),
        created_at_ms: task.created_at_ms,
        started_at_ms: task.started_at_ms,
        finished_at_ms: task.finished_at_ms,
    }
}

/// The tasks in the log at `path` as the screens get them, newest first. Also what the desktop shows for this computer.
pub fn agent_task_dtos(path: &Path) -> Vec<AgentTaskDto> {
    read_agent_tasks(path).into_iter().map(to_dto).collect()
}

/// Answers `ControlAgentTask`: the owner, with the pairing key, pauses, resumes or stops a task running on this hub; the reply is the
/// updated list, or a `TaskError`. Takes the same second wait on a wrong key as every other change.
pub async fn handle_control_agent_task(
    log: Option<&Path>,
    is_owner: bool,
    auth_key: &str,
    request_id: u64,
    pairing_key: &str,
    task_id: &str,
    action: &str,
) -> ServerMessage {
    let error = |message: &str, auth_rejected: bool| ServerMessage::TaskError { request_id, message: message.to_string(), auth_rejected };
    let (Some(path), true) = (log, is_owner) else {
        return error("this hub doesn't keep the delegated tasks for you", false);
    };
    if !keys_match(pairing_key, auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return error("wrong pairing key", true);
    }
    let result = TaskControl::parse(action).and_then(|action| control_agent_task(path, task_id, action).map(|()| action));
    match result {
        Ok(action) => {
            // A stop is carried out by the task's own runtime a moment later: wait for it to be written down, so the list that answers says "cancelled".
            if action == TaskControl::Cancel {
                for _ in 0..50 {
                    if !warden_core::jobs::task_controls().is_controllable(task_id) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
            handle_list_agent_tasks(Some(path), true, request_id)
        }
        Err(message) => error(&message, false),
    }
}

/// Answers `ListAgentTasks`. `log` is `None` on a hub that keeps none.
pub fn handle_list_agent_tasks(log: Option<&Path>, is_owner: bool, request_id: u64) -> ServerMessage {
    let tasks = match log {
        Some(path) if is_owner => agent_task_dtos(path),
        _ => Vec::new(),
    };
    ServerMessage::AgentTaskList { request_id, tasks }
}
