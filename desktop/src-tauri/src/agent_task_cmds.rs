//! The work agents delegated to each other (P123), for the desktop's "Agent work" screen when it shows this computer: the same log
//! the engine writes (`warden_bootstrap::agent_tasks`), read from disk. A hub in use is asked over the wire instead (`listAgentTasks`).

use warden_bootstrap::agent_tasks::TaskControl;
use warden_server::agent_task_list::agent_task_dtos;
use warden_server_protocol::protocol::AgentTaskDto;

/// The tasks of this computer, newest first. No log yet is no tasks.
#[tauri::command]
pub fn list_agent_tasks() -> Vec<AgentTaskDto> {
    warden_bootstrap::resolve_agent_tasks_path(std::env::var("WARDEN_AGENT_TASKS").ok()).map(|path| agent_task_dtos(&path)).unwrap_or_default()
}

/// Pauses, resumes or stops (`action`: `pause`, `resume` or `cancel`) a task of this computer's engine, with the subtasks below it.
/// No pairing key: the engine is this app's own. The refusal says why (not running here, already ended, not in that state).
#[tauri::command]
pub async fn control_agent_task(task_id: String, action: String) -> Result<(), String> {
    let path = warden_bootstrap::resolve_agent_tasks_path(std::env::var("WARDEN_AGENT_TASKS").ok()).ok_or_else(|| "there is no task log on this computer".to_string())?;
    let action = TaskControl::parse(&action)?;
    control_agent_task_in(&path, &task_id, action)?;
    // A stop is carried out by the task's own runtime a moment later: wait for it to be written down, so the list the screen reloads says "cancelled".
    if action == TaskControl::Cancel {
        for _ in 0..50 {
            if !warden_core::jobs::task_controls().is_controllable(&task_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
    Ok(())
}

fn control_agent_task_in(path: &std::path::Path, task_id: &str, action: TaskControl) -> Result<(), String> {
    warden_bootstrap::agent_tasks::control_agent_task(path, task_id, action)
}
