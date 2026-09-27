//! Scheduled tasks (P92) on the hub: a loop that wakes up every so often, re-reads `[[tasks]]` from
//! the config file (so a pause or an edit takes effect without a restart, like the agents) and runs
//! whatever is due, each in its own task. Only a hub started with `--run-tasks` runs this — the file
//! syncs, so without that switch two hubs would run every task twice.
//!
//! The rules live in `warden_bootstrap::tasks`; this file only drives them and tells every
//! connected device when a task's conversation changed.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;
use warden_bootstrap::load_config_from_path;
use warden_bootstrap::tasks::{conversation_id, run_task, TaskStore};

use crate::settings::{SettingsHost, SharedOrchestrator};

/// How often the loop looks for due tasks. A task runs at most this late.
pub const DEFAULT_TICK: Duration = Duration::from_secs(30);

pub(crate) async fn scheduler_loop(
    store: TaskStore,
    settings: Arc<dyn SettingsHost>,
    shared: SharedOrchestrator,
    changes: broadcast::Sender<String>,
    tick: Duration,
) {
    // Tasks still running: a run longer than its interval is skipped, not stacked.
    let running: Arc<Mutex<HashSet<String>>> = Arc::default();
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // The first tick is immediate: runs missed while the hub was down happen right away.
        interval.tick().await;
        run_due(&store, settings.as_ref(), &shared, &changes, &running);
    }
}

fn run_due(store: &TaskStore, settings: &dyn SettingsHost, shared: &SharedOrchestrator, changes: &broadcast::Sender<String>, running: &Arc<Mutex<HashSet<String>>>) {
    let config_path = settings.config_path();
    let config = match load_config_from_path(&config_path, false) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("warden-server: scheduled tasks: can't read {}: {err:#}", config_path.display());
            return;
        }
    };
    let now = now_millis();
    let due = match store.claim_due(&config.tasks, now) {
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
        if !running.lock().unwrap_or_else(|e| e.into_inner()).insert(task.id.clone()) {
            eprintln!("warden-server: task '{}' is due but its last run hasn't finished, skipping this one", task.id);
            continue;
        }
        let (store, config, config_path, base) = (store.clone(), config.clone(), config_path.clone(), base.clone());
        let (changes, running) = (changes.clone(), running.clone());
        tokio::spawn(async move {
            eprintln!("warden-server: running task '{}'", task.id);
            let result = run_task(&base, &config, Some(&config_path), &task, &store.conversations_dir(), now).await;
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
            if let Err(err) = store.record_finish(&task.id, now_millis(), error) {
                eprintln!("warden-server: task '{}': can't save its run state: {err:#}", task.id);
            }
            running.lock().unwrap_or_else(|e| e.into_inner()).remove(&task.id);
            // Nobody may be connected; that's fine.
            let _ = changes.send(conversation_id(&task.id));
        });
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}
