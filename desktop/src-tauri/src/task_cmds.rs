//! Tauri commands backing the Tasks screen (P92): the desktop's own `[[tasks]]` in `config.toml`,
//! the same list/create/edit/pause/remove/run as `warden-server tasks` and the web. Stateless like
//! `api_key_cmds.rs`: every call rereads the config and the run state.
//!
//! Whether this machine runs them on schedule is its own switch, in `hub-local.json` (the config
//! syncs whole): it only matters while the embedded hub runs, and flipping it restarts that hub.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use tauri::State;
use warden_bootstrap::tasks::{
    conversation_id, default_hub_local_path, load_hub_local, remove_task, save_hub_local, set_task_enabled, task_infos, upsert_task, HubLocalConfig, TaskStore,
};
use warden_bootstrap::{load_config_from_path, load_conversation, save_config, ChatRole, FileConfig};
use warden_server::scheduler::TaskRunner;
use warden_server_protocol::protocol::{TaskDto, TaskInfoDto};

use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskListPayload {
    tasks: Vec<TaskInfoDto>,
    /// This machine's switch.
    run_here: bool,
    /// The embedded hub is running — the switch only does something then.
    hub_running: bool,
}

/// One message of a task's conversation, for the read-only history.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskMessage {
    role: &'static str,
    content: String,
    created_at: i64,
}

fn config_path() -> Result<PathBuf, String> {
    warden_bootstrap::default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

fn store() -> Result<TaskStore, String> {
    warden_bootstrap::default_server_tasks_dir().map(TaskStore::new).ok_or_else(|| "could not determine the OS config directory".to_string())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// Whether this machine's embedded hub runs the tasks on schedule. An unreadable file counts as off.
pub(crate) fn run_tasks_here() -> bool {
    default_hub_local_path().and_then(|path| load_hub_local(&path).ok()).is_some_and(|local| local.run_tasks)
}

fn list(state: &AppState) -> Result<TaskListPayload, String> {
    let config = load_config_from_path(&config_path()?, false).map_err(|e| format!("{e:#}"))?;
    let tasks = task_infos(&config.tasks, &store()?, now_millis()).map_err(|e| format!("{e:#}"))?;
    let hub_running = state.embedded_server.lock().unwrap().is_some();
    Ok(TaskListPayload { tasks, run_here: run_tasks_here(), hub_running })
}

/// Loads the config, applies `change` and saves it — `[[tasks]]` only; the scheduler rereads it.
fn change(state: &AppState, change: impl FnOnce(&mut FileConfig) -> anyhow::Result<()>) -> Result<TaskListPayload, String> {
    let path = config_path()?;
    let mut config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    change(&mut config).map_err(|e| format!("{e:#}"))?;
    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;
    list(state)
}

#[tauri::command]
pub fn list_tasks(state: State<'_, AppState>) -> Result<TaskListPayload, String> {
    list(&state)
}

/// Creates a task, or replaces `original_id` with it (a rename when the ids differ).
#[tauri::command]
pub fn save_task(state: State<'_, AppState>, original_id: Option<String>, task: TaskDto) -> Result<TaskListPayload, String> {
    change(&state, |config| upsert_task(config, original_id.as_deref(), task.into()))
}

#[tauri::command]
pub fn set_task_enabled_cmd(state: State<'_, AppState>, id: String, enabled: bool) -> Result<TaskListPayload, String> {
    change(&state, |config| set_task_enabled(config, &id, enabled))
}

#[tauri::command]
pub fn delete_task(state: State<'_, AppState>, id: String) -> Result<TaskListPayload, String> {
    change(&state, |config| remove_task(config, &id))
}

/// Used for "Run now" while the embedded hub is off.
static LOCAL_RUNNER: OnceLock<TaskRunner> = OnceLock::new();

/// Starts a run in the background and answers at once; the screen polls while a task is running.
/// With the embedded hub up, through its runner, so the hub's devices hear when it's done.
#[tauri::command]
pub async fn run_task_now(state: State<'_, AppState>, id: String) -> Result<TaskListPayload, String> {
    let path = config_path()?;
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    let task = config.tasks.iter().find(|t| t.id == id).cloned().ok_or_else(|| format!("no task named '{id}'"))?;
    task.schedule().map_err(|e| format!("{e:#}"))?;
    let hub = state.embedded_server.lock().unwrap().as_ref().map(|hub| (hub.task_runner.clone(), hub.orchestrator.current()));
    let (runner, base) = match hub {
        Some((Some(runner), base)) => (runner, base),
        _ => {
            let base = state.orchestrator.lock().unwrap().clone()?;
            let store = store()?;
            let runner = LOCAL_RUNNER.get_or_init(|| TaskRunner::new(store, tokio::sync::broadcast::channel(1).0, false));
            (runner.clone(), Arc::new(base))
        }
    };
    if !runner.start(base, Arc::new(config), path, task, true) {
        return Err(format!("task '{id}' is still running"));
    }
    list(&state)
}

/// The task's conversation, oldest first — read-only on this screen. Empty when this machine never
/// ran it (another hub may have: its devices see it there).
#[tauri::command]
pub fn task_history(id: String) -> Result<Vec<TaskMessage>, String> {
    let conversation = load_conversation(&store()?.conversations_dir(), &conversation_id(&id)).map_err(|e| format!("{e:#}"))?;
    Ok(conversation
        .map(|c| c.messages)
        .unwrap_or_default()
        .into_iter()
        .map(|m| TaskMessage { role: if m.role == ChatRole::User { "user" } else { "assistant" }, content: m.content, created_at: m.created_at })
        .collect())
}

/// Turns this machine's switch on or off, restarting the embedded hub (if it runs) to apply it.
#[tauri::command]
pub async fn set_run_tasks_here(state: State<'_, AppState>, enabled: bool) -> Result<TaskListPayload, String> {
    let path = default_hub_local_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let mut local: HubLocalConfig = load_hub_local(&path).unwrap_or_default();
    local.run_tasks = enabled;
    save_hub_local(&path, &local).map_err(|e| format!("{e:#}"))?;
    crate::server_cmds::restart_embedded_server(&state).await?;
    list(&state)
}
