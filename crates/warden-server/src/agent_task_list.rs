//! The work agents delegated to each other (P123), for the screens: `ListAgentTasks` reads the log the hub's orchestrator writes
//! (`warden_bootstrap::agent_tasks`). Read only, and the owner's: tasks hold what agents were asked and what they answered, which a
//! member has no business reading in the owner's log, so a member gets an empty list.

use std::path::Path;

use warden_bootstrap::agent_tasks::{read_agent_tasks, AgentTask};
use warden_server_protocol::protocol::AgentTaskDto;
use warden_server_protocol::ServerMessage;

fn to_dto(task: AgentTask) -> AgentTaskDto {
    AgentTaskDto {
        id: task.id,
        group: task.group,
        owner: task.owner,
        assignee: task.assignee,
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

/// Answers `ListAgentTasks`. `log` is `None` on a hub that keeps none.
pub fn handle_list_agent_tasks(log: Option<&Path>, is_owner: bool, request_id: u64) -> ServerMessage {
    let tasks = match log {
        Some(path) if is_owner => agent_task_dtos(path),
        _ => Vec::new(),
    };
    ServerMessage::AgentTaskList { request_id, tasks }
}
