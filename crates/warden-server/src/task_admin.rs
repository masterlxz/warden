//! Scheduled tasks (P92) from a client: the same list/create/edit/pause/remove/run as
//! `warden-server tasks`, for the web's Tasks screen. Listing is open to any paired device, like the
//! device list; every change asks for the pairing key again, with the same 1 s wait and the same
//! per-hub lock as a settings save — a task runs an agent on its own, with its tools and its spend.
//!
//! Changes write `[[tasks]]` in the config file and nothing else: the scheduler re-reads it, so the
//! orchestrator isn't rebuilt.

use std::sync::Arc;

use warden_bootstrap::tasks::{remove_task, set_task_enabled, task_infos, upsert_task};
use warden_bootstrap::{load_config_from_path, save_config};
use warden_server_protocol::protocol::TaskDto;
use warden_server_protocol::ServerMessage;

use crate::scheduler::TaskRunner;
use crate::settings::{keys_match, SettingsHost, SharedOrchestrator, WRONG_KEY_DELAY};

fn task_error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::TaskError { request_id, message, auth_rejected }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn list(runner: &TaskRunner, settings: &dyn SettingsHost, request_id: u64) -> anyhow::Result<ServerMessage> {
    let config = load_config_from_path(&settings.config_path(), false)?;
    let tasks = task_infos(&config.tasks, runner.store(), now_millis())?;
    Ok(ServerMessage::TaskList { request_id, tasks, runs_here: runner.runs_here() })
}

fn parts<'a>(runner: Option<&'a TaskRunner>, settings: Option<&'a dyn SettingsHost>) -> Result<(&'a TaskRunner, &'a dyn SettingsHost), String> {
    let runner = runner.ok_or_else(|| "this hub keeps no scheduled tasks".to_string())?;
    let settings = settings.ok_or_else(|| "this hub has no settings file, so it has no scheduled tasks".to_string())?;
    Ok((runner, settings))
}

/// Answers `ListTasks`.
pub fn handle_list_tasks(runner: Option<&TaskRunner>, settings: Option<&dyn SettingsHost>, request_id: u64) -> ServerMessage {
    match parts(runner, settings) {
        Ok((runner, settings)) => list(runner, settings, request_id).unwrap_or_else(|err| task_error(request_id, format!("{err:#}"), false)),
        Err(message) => task_error(request_id, message, false),
    }
}

/// What `SaveTask`/`SetTaskEnabled`/`DeleteTask`/`RunTask` asks for.
pub enum TaskChange {
    Save { original_id: Option<String>, task: TaskDto },
    SetEnabled { id: String, enabled: bool },
    Delete { id: String },
    Run { id: String },
}

/// Everything a task change needs from the connection.
pub struct TaskAccess<'a> {
    pub runner: Option<&'a TaskRunner>,
    pub settings: Option<&'a dyn SettingsHost>,
    pub shared: &'a SharedOrchestrator,
    pub lock: &'a tokio::sync::Mutex<()>,
    pub auth_key: &'a str,
}

/// Answers a task change with the updated `TaskList`. A run is started in the background; the
/// task's conversation changing (`ConversationsChanged`) tells when it's done.
pub async fn handle_task_change(access: &TaskAccess<'_>, request_id: u64, pairing_key: &str, change: TaskChange) -> ServerMessage {
    let (runner, settings) = match parts(access.runner, access.settings) {
        Ok(parts) => parts,
        Err(message) => return task_error(request_id, message, false),
    };
    let _serialized = access.lock.lock().await;
    if !keys_match(pairing_key, access.auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return task_error(request_id, "wrong pairing key".to_string(), true);
    }
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<()> {
        let mut config = load_config_from_path(&config_path, false)?;
        match change {
            TaskChange::Save { original_id, task } => upsert_task(&mut config, original_id.as_deref(), task.into())?,
            TaskChange::SetEnabled { id, enabled } => set_task_enabled(&mut config, &id, enabled)?,
            TaskChange::Delete { id } => remove_task(&mut config, &id)?,
            TaskChange::Run { id } => {
                let task = config.tasks.iter().find(|t| t.id == id).cloned().ok_or_else(|| anyhow::anyhow!("no task named '{id}'"))?;
                task.schedule()?;
                let started = runner.start(access.shared.current(), Arc::new(config), config_path.clone(), task, true);
                anyhow::ensure!(started, "task '{id}' is still running");
                return Ok(());
            }
        }
        save_config(&config_path, &config)
    })();
    match result.and_then(|()| list(runner, settings, request_id)) {
        Ok(reply) => reply,
        Err(err) => task_error(request_id, format!("{err:#}"), false),
    }
}
