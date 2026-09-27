//! Scheduled tasks (P92) on the hub: a loop that wakes up every so often, re-reads `[[tasks]]` from
//! the config file (so a pause or an edit takes effect without a restart, like the agents) and runs
//! whatever is due, each in its own task. Only a hub started with `--run-tasks` (or the desktop's
//! switch) runs the loop — the file syncs, so without that switch two hubs would run every task
//! twice. A "run now" from a screen goes through the same `TaskRunner`, so it never overlaps a
//! scheduled run of the same task.
//!
//! The rules live in `warden_bootstrap::tasks`; this file only drives them and tells every
//! connected device when a task's conversation changed.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;
use warden_bootstrap::tasks::{conversation_id, run_task, TaskStore};
use warden_bootstrap::{load_config_from_path, FileConfig, TaskConfig};
use warden_core::orchestrator::Orchestrator;

use crate::settings::{SettingsHost, SharedOrchestrator};

/// How often the loop looks for due tasks. A task runs at most this late.
pub const DEFAULT_TICK: Duration = Duration::from_secs(30);

/// Runs tasks for one hub: their store, the ones running now, and where to announce a finished run.
#[derive(Clone)]
pub struct TaskRunner {
    store: TaskStore,
    running: Arc<Mutex<HashSet<String>>>,
    changes: broadcast::Sender<String>,
    runs_here: bool,
}

impl TaskRunner {
    pub fn new(store: TaskStore, changes: broadcast::Sender<String>, runs_here: bool) -> Self {
        Self { store, running: Arc::default(), changes, runs_here }
    }

    pub fn store(&self) -> &TaskStore {
        &self.store
    }

    /// Whether this hub runs the tasks on schedule.
    pub fn runs_here(&self) -> bool {
        self.runs_here
    }

    /// Starts `task` in the background, or returns `false` when its last run hasn't finished.
    /// `manual`: a "run now", which also records the start (a scheduled run was already claimed).
    pub fn start(&self, base: Arc<Orchestrator>, config: Arc<FileConfig>, config_path: PathBuf, task: TaskConfig, manual: bool) -> bool {
        if !self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(task.id.clone()) {
            return false;
        }
        let now = now_millis();
        if manual {
            if let Err(err) = self.store.mark_started(&task, now) {
                eprintln!("warden-server: task '{}': can't save its run state: {err:#}", task.id);
            }
        }
        let this = self.clone();
        tokio::spawn(async move {
            eprintln!("warden-server: running task '{}'", task.id);
            let result = run_task(&base, &config, Some(&config_path), &task, &this.store.conversations_dir(), now).await;
            let error = match &result {
                Ok(_) => {
                    eprintln!("warden-server: task '{}' done", task.id);
                    None
                }
                Err(err) => {
                    eprintln!("warden-server: task '{}' failed: {err:#}", task.id);
                    Some(format!("{err:#}"))
                }
            };
            if let Err(err) = this.store.record_finish(&task.id, now_millis(), error) {
                eprintln!("warden-server: task '{}': can't save its run state: {err:#}", task.id);
            }
            this.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&task.id);
            // Nobody may be connected; that's fine.
            let _ = this.changes.send(conversation_id(&task.id));
        });
        true
    }
}

pub(crate) async fn scheduler_loop(runner: TaskRunner, settings: Arc<dyn SettingsHost>, shared: SharedOrchestrator, tick: Duration) {
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // The first tick is immediate: runs missed while the hub was down happen right away.
        interval.tick().await;
        run_due(&runner, settings.as_ref(), &shared);
    }
}

fn run_due(runner: &TaskRunner, settings: &dyn SettingsHost, shared: &SharedOrchestrator) {
    let config_path = settings.config_path();
    let config = match load_config_from_path(&config_path, false) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("warden-server: scheduled tasks: can't read {}: {err:#}", config_path.display());
            return;
        }
    };
    let due = match runner.store.claim_due(&config.tasks, now_millis()) {
        Ok(due) => due,
        Err(err) => {
            eprintln!("warden-server: scheduled tasks: can't update the run state: {err:#}");
            return;
        }
    };
    if due.is_empty() {
        return;
    }
    let config = Arc::new(config);
    let base = shared.current();
    for task in due {
        let id = task.id.clone();
        if !runner.start(base.clone(), config.clone(), config_path.clone(), task, false) {
            eprintln!("warden-server: task '{id}' is due but its last run hasn't finished, skipping this one");
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}
