//! The work agents delegated to each other (P123), for the desktop's "Agent work" screen when it shows this computer: the same log
//! the engine writes (`warden_bootstrap::agent_tasks`), read from disk. A hub in use is asked over the wire instead (`listAgentTasks`).

use warden_server::agent_task_list::agent_task_dtos;
use warden_server_protocol::protocol::AgentTaskDto;

/// The tasks of this computer, newest first. No log yet is no tasks.
#[tauri::command]
pub fn list_agent_tasks() -> Vec<AgentTaskDto> {
    warden_bootstrap::resolve_agent_tasks_path(std::env::var("WARDEN_AGENT_TASKS").ok()).map(|path| agent_task_dtos(&path)).unwrap_or_default()
}
