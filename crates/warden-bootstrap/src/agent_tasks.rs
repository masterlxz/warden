//! The work agents hand to each other, kept as records (P123). A delegation in the background (`delegate_task` or
//! `delegate_to_agent` with `background: true`) is written down as a task when it is queued, when it starts and when it
//! ends, so a screen can show who is doing what, how far along a manager's batch is, and what it cost — after the turn that
//! started it is over.
//!
//! One append-only log of JSON lines (`agent_tasks.jsonl`, next to `spend_ledger.jsonl`), shared by every Warden process on the
//! machine. Reading folds the events into the latest state of each task; a task that was last seen pending or running long
//! ago is shown as cancelled, since whoever was running it is gone.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use warden_core::jobs::{TaskOutcome, TaskRecorder, TaskSpec};
use warden_core::model::Usage;

/// The most tasks a read hands back (the newest ones) and keeps when the log is compacted.
pub const MAX_TASKS: usize = 200;
/// Past this size the log is rewritten with only the tasks that are kept.
const COMPACT_ABOVE_BYTES: u64 = 1024 * 1024;
/// A task last seen pending or running this long ago is shown as cancelled.
pub const STALE_AFTER_MS: u64 = 6 * 60 * 60 * 1000;
const MAX_OBJECTIVE_CHARS: usize = 2000;
const MAX_RESULT_CHARS: usize = 4000;

/// Where a task is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Queued, waiting for a free slot under the limit of tasks running at once.
    Pending,
    Running,
    /// The task's agent is waiting for another agent: a subtask it started (P123).
    Waiting,
    /// A person paused it from a screen: it finishes the model call it is in and waits to be resumed (P123).
    Paused,
    Done,
    Failed,
    /// The turn ended (or was cancelled) before it finished, or its runner disappeared.
    Cancelled,
}

impl TaskState {
    pub fn is_finished(self) -> bool {
        matches!(self, TaskState::Done | TaskState::Failed | TaskState::Cancelled)
    }

    /// The word screens and the wire use.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Pending => "pending",
            TaskState::Running => "running",
            TaskState::Waiting => "waiting",
            TaskState::Paused => "paused",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }
}

/// One delegated task, as of the last event written about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTask {
    pub id: String,
    /// Every task one turn started shares it: the batch a progress is counted over.
    pub group: String,
    /// The agent that delegated.
    pub owner: Option<String>,
    /// The agent that does the work, or the name given to a temporary helper.
    pub assignee: String,
    /// The task this one is a subtask of, when its agent started it from inside another task (P123).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub objective: String,
    /// The provider or combo chosen for this task, if the delegating agent chose one.
    pub model: Option<String>,
    pub channel: String,
    pub state: TaskState,
    /// What it produced, when it is done.
    pub result: Option<String>,
    /// Why it failed or was cancelled.
    pub error: Option<String>,
    pub usage: Option<Usage>,
    pub created_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
}

/// What a person can do to a task from a screen (P123).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskControl {
    Pause,
    Resume,
    Cancel,
}

impl TaskControl {
    /// The word screens send: `pause`, `resume` or `cancel`.
    pub fn parse(action: &str) -> Result<Self, String> {
        match action {
            "pause" => Ok(TaskControl::Pause),
            "resume" => Ok(TaskControl::Resume),
            "cancel" => Ok(TaskControl::Cancel),
            other => Err(format!("unknown action '{other}' — use pause, resume or cancel")),
        }
    }
}

/// Pauses, resumes or stops the task `id` of the log at `log` when it is running in this process, with the subtasks below it. The refusals
/// say why: not running here (another process, or it already ended), or not in a state the action fits.
pub fn control_agent_task(log: &Path, id: &str, action: TaskControl) -> Result<(), String> {
    let controls = warden_core::jobs::task_controls();
    let task = read_agent_tasks(log).into_iter().find(|t| t.id == id).ok_or_else(|| format!("no task '{id}'"))?;
    if task.state.is_finished() {
        return Err(format!("the task already ended ({})", task.state.as_str()));
    }
    if !controls.is_controllable(id) {
        return Err("this task isn't running on this machine, so it can't be controlled from here".to_string());
    }
    match (action, task.state) {
        (TaskControl::Pause, TaskState::Paused) => return Err("the task is already paused".to_string()),
        (TaskControl::Pause, TaskState::Pending) => return Err("the task hasn't started: cancel it, or wait for it to start".to_string()),
        (TaskControl::Resume, state) if state != TaskState::Paused => return Err("the task isn't paused".to_string()),
        _ => {}
    }
    let done = match action {
        TaskControl::Pause => controls.pause(id),
        TaskControl::Resume => controls.resume(id),
        TaskControl::Cancel => controls.cancel(id),
    };
    done.then_some(()).ok_or_else(|| "the task ended just now".to_string())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
enum Event {
    /// The whole task as it was at this moment: how a task is created, and how a compacted log keeps it.
    Task { task: Box<AgentTask> },
    Running { id: String, at: u64 },
    Waiting { id: String, at: u64 },
    Resumed { id: String, at: u64 },
    Paused { id: String, at: u64 },
    Unpaused { id: String, at: u64 },
    Finished { id: String, at: u64, state: TaskState, result: Option<String>, error: Option<String>, usage: Option<Usage> },
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

/// Where the log lives, next to `config.toml`.
pub fn default_agent_tasks_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("agent_tasks.jsonl"))
}

/// The counter at the end of a task's id. One per process, not per recorder: the registry that lets a person pause or stop a task
/// (`warden_core::jobs::task_controls`) is process-wide and keyed by id, so two recorders in one process (a hub and a local desktop, two
/// tests) must not make the same id in the same millisecond.
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Writes the tasks the background jobs run (P123). Cheap: one short line appended per event.
pub struct FileTaskRecorder {
    path: PathBuf,
    /// Keeps this process's appends and the compaction from interleaving.
    write: Mutex<()>,
}

impl FileTaskRecorder {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), write: Mutex::new(()) }
    }

    fn append(&self, event: &Event) {
        let Ok(mut line) = serde_json::to_string(event) else { return };
        line.push('\n');
        let _held = self.write.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let written = OpenOptions::new().create(true).append(true).open(&self.path).and_then(|mut file| file.write_all(line.as_bytes()));
        // A log that can't be written must never stop the work it describes.
        if written.is_ok() && std::fs::metadata(&self.path).is_ok_and(|m| m.len() > COMPACT_ABOVE_BYTES) {
            self.compact();
        }
    }

    /// Rewrites the log as one snapshot per task kept. Called with the write lock held.
    fn compact(&self) {
        let tasks = read_agent_tasks(&self.path);
        let mut text = String::new();
        for task in tasks.into_iter().rev() {
            if let Ok(line) = serde_json::to_string(&Event::Task { task: Box::new(task) }) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        let temp = self.path.with_extension("jsonl.tmp");
        if std::fs::write(&temp, text).is_ok() {
            let _ = std::fs::rename(&temp, &self.path);
        }
    }
}

impl TaskRecorder for FileTaskRecorder {
    fn created(&self, spec: &TaskSpec) -> String {
        let now = now_ms();
        let id = format!("at-{now}-{}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed));
        self.append(&Event::Task {
            task: Box::new(AgentTask {
                id: id.clone(),
                group: spec.group.clone(),
                owner: spec.owner.clone(),
                assignee: spec.assignee.clone(),
                parent_id: spec.parent.clone(),
                objective: clip(&spec.objective, MAX_OBJECTIVE_CHARS),
                model: spec.model.clone(),
                channel: spec.channel.clone(),
                state: TaskState::Pending,
                result: None,
                error: None,
                usage: None,
                created_at_ms: now,
                started_at_ms: None,
                finished_at_ms: None,
            }),
        });
        id
    }

    fn running(&self, id: &str) {
        self.append(&Event::Running { id: id.to_string(), at: now_ms() });
    }

    fn waiting(&self, id: &str) {
        self.append(&Event::Waiting { id: id.to_string(), at: now_ms() });
    }

    fn resumed(&self, id: &str) {
        self.append(&Event::Resumed { id: id.to_string(), at: now_ms() });
    }

    fn paused(&self, id: &str) {
        self.append(&Event::Paused { id: id.to_string(), at: now_ms() });
    }

    fn unpaused(&self, id: &str) {
        self.append(&Event::Unpaused { id: id.to_string(), at: now_ms() });
    }

    fn finished(&self, id: &str, outcome: TaskOutcome) {
        let (state, result, error, usage) = match outcome {
            TaskOutcome::Stopped => (TaskState::Cancelled, None, Some("stopped by a person".to_string()), None),
            TaskOutcome::Done { result, usage } => (TaskState::Done, Some(clip(&result, MAX_RESULT_CHARS)), None, usage),
            TaskOutcome::Failed { error } => (TaskState::Failed, None, Some(clip(&error, MAX_RESULT_CHARS)), None),
            TaskOutcome::Cancelled => (TaskState::Cancelled, None, Some("the turn ended before it finished".to_string()), None),
        };
        self.append(&Event::Finished { id: id.to_string(), at: now_ms(), state, result, error, usage });
    }
}

/// The tasks in the log at `path`, newest first, at most `MAX_TASKS`. A missing file is no tasks, a line that doesn't parse is skipped.
pub fn read_agent_tasks(path: &Path) -> Vec<AgentTask> {
    read_agent_tasks_at(path, now_ms())
}

fn read_agent_tasks_at(path: &Path, now: u64) -> Vec<AgentTask> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut tasks: Vec<AgentTask> = Vec::new();
    for event in text.lines().filter_map(|line| serde_json::from_str::<Event>(line).ok()) {
        match event {
            Event::Task { task } => match tasks.iter_mut().find(|t| t.id == task.id) {
                Some(existing) => *existing = *task,
                None => tasks.push(*task),
            },
            Event::Running { id, at } => {
                if let Some(task) = tasks.iter_mut().find(|t| t.id == id && !t.state.is_finished()) {
                    task.state = TaskState::Running;
                    task.started_at_ms = Some(at);
                }
            }
            // A pause outranks a wait: the task stays paused until a person lifts it.
            Event::Waiting { id, at } => {
                if let Some(task) = tasks.iter_mut().find(|t| t.id == id && !t.state.is_finished() && t.state != TaskState::Paused) {
                    task.state = TaskState::Waiting;
                    task.started_at_ms = task.started_at_ms.or(Some(at));
                }
            }
            Event::Paused { id, at } => {
                if let Some(task) = tasks.iter_mut().find(|t| t.id == id && !t.state.is_finished()) {
                    task.state = TaskState::Paused;
                    task.started_at_ms = task.started_at_ms.or(Some(at));
                }
            }
            Event::Unpaused { id, .. } => {
                if let Some(task) = tasks.iter_mut().find(|t| t.id == id && t.state == TaskState::Paused) {
                    task.state = TaskState::Running;
                }
            }
            Event::Resumed { id, .. } => {
                if let Some(task) = tasks.iter_mut().find(|t| t.id == id && t.state == TaskState::Waiting) {
                    task.state = TaskState::Running;
                }
            }
            Event::Finished { id, at, state, result, error, usage } => {
                if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
                    task.state = state;
                    task.finished_at_ms = Some(at);
                    task.result = result;
                    task.error = error;
                    task.usage = usage;
                }
            }
        }
    }
    for task in tasks.iter_mut().filter(|t| !t.state.is_finished()) {
        let last_seen = task.started_at_ms.unwrap_or(task.created_at_ms);
        if now.saturating_sub(last_seen) > STALE_AFTER_MS {
            task.state = TaskState::Cancelled;
            task.error = Some("interrupted: nothing was recorded about it for a long time".to_string());
        }
    }
    // Newest first; tasks made in the same millisecond by one process keep their order through the counter at the end of the id.
    let counter = |id: &str| id.rsplit('-').next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
    tasks.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms).then_with(|| counter(&b.id).cmp(&counter(&a.id))).then_with(|| b.id.cmp(&a.id)));
    tasks.truncate(MAX_TASKS);
    tasks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log() -> PathBuf {
        std::env::temp_dir().join(format!("warden-agent-tasks-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())).join("agent_tasks.jsonl")
    }

    fn spec(assignee: &str, group: &str) -> TaskSpec {
        TaskSpec {
            group: group.into(),
            owner: Some("chief".into()),
            assignee: assignee.into(),
            objective: "write the thing".into(),
            model: Some("fast".into()),
            channel: "desktop".into(),
            parent: None,
        }
    }

    #[test]
    fn two_recorders_in_one_process_never_make_the_same_id() {
        let (first, second) = (FileTaskRecorder::new(temp_log()), FileTaskRecorder::new(temp_log()));
        let ids: Vec<String> = (0..20).flat_map(|_| [first.created(&spec("a", "g")), second.created(&spec("b", "g"))]).collect();
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "the registry that stops a task is keyed by id for the whole process");
    }

    #[test]
    fn a_task_waiting_for_a_subtask_is_shown_waiting_and_runs_again_when_it_resumes() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let manager = recorder.created(&spec("manager", "g"));
        recorder.running(&manager);
        let helper = recorder.created(&TaskSpec { parent: Some(manager.clone()), ..spec("helper", "g") });
        recorder.running(&helper);

        recorder.waiting(&manager);
        let tasks = read_agent_tasks(&log);
        let state_of = |tasks: &[AgentTask], id: &str| tasks.iter().find(|t| t.id == id).unwrap().state;
        assert_eq!(state_of(&tasks, &manager), TaskState::Waiting);
        assert_eq!(state_of(&tasks, &helper), TaskState::Running);
        assert_eq!(tasks.iter().find(|t| t.id == helper).unwrap().parent_id.as_deref(), Some(manager.as_str()));
        assert_eq!(tasks.iter().find(|t| t.id == manager).unwrap().parent_id, None);

        recorder.finished(&helper, TaskOutcome::Done { result: "ok".into(), usage: None });
        recorder.resumed(&manager);
        assert_eq!(state_of(&read_agent_tasks(&log), &manager), TaskState::Running);

        recorder.finished(&manager, TaskOutcome::Done { result: "all".into(), usage: None });
        assert_eq!(state_of(&read_agent_tasks(&log), &manager), TaskState::Done);
    }

    #[test]
    fn a_task_a_person_pauses_stays_paused_through_a_wait_until_it_is_lifted_and_one_they_stop_says_so() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let state = |id: &str| read_agent_tasks(&log).into_iter().find(|t| t.id == id).unwrap();
        let id = recorder.created(&spec("a", "g"));
        recorder.running(&id);

        recorder.paused(&id);
        assert_eq!(state(&id).state, TaskState::Paused);
        // The agent was waiting for a subtask that ends meanwhile: still paused.
        recorder.waiting(&id);
        recorder.resumed(&id);
        assert_eq!(state(&id).state, TaskState::Paused);
        recorder.unpaused(&id);
        assert_eq!(state(&id).state, TaskState::Running);
        assert!(!TaskState::Paused.is_finished() && TaskState::Paused.as_str() == "paused");

        recorder.paused(&id);
        recorder.finished(&id, TaskOutcome::Stopped);
        let stopped = state(&id);
        assert_eq!((stopped.state, stopped.error.as_deref()), (TaskState::Cancelled, Some("stopped by a person")));
        // A late unpause or pause doesn't reopen it.
        recorder.unpaused(&id);
        recorder.paused(&id);
        assert_eq!(state(&id).state, TaskState::Cancelled);
    }

    #[test]
    fn a_resume_that_arrives_late_does_not_reopen_a_finished_task_and_a_wait_after_the_end_is_ignored() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let id = recorder.created(&spec("a", "g"));
        recorder.running(&id);
        recorder.finished(&id, TaskOutcome::Cancelled);
        recorder.waiting(&id);
        recorder.resumed(&id);
        assert_eq!(read_agent_tasks(&log)[0].state, TaskState::Cancelled);
    }

    #[test]
    fn a_task_left_waiting_long_ago_is_shown_as_cancelled() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let id = recorder.created(&spec("a", "g"));
        recorder.running(&id);
        recorder.waiting(&id);
        let tasks = read_agent_tasks_at(&log, now_ms() + STALE_AFTER_MS + 1000);
        assert_eq!(tasks[0].state, TaskState::Cancelled);
        assert_eq!(read_agent_tasks(&log)[0].state, TaskState::Waiting);
    }

    #[test]
    fn compacting_keeps_the_parent_and_a_waiting_state() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let manager = recorder.created(&spec("manager", "g"));
        recorder.running(&manager);
        recorder.created(&TaskSpec { parent: Some(manager.clone()), ..spec("helper", "g") });
        recorder.waiting(&manager);
        let before = read_agent_tasks(&log);
        {
            let _held = recorder.write.lock().unwrap();
            recorder.compact();
        }
        assert_eq!(read_agent_tasks(&log), before);
    }

    #[test]
    fn a_task_goes_from_pending_to_running_to_done_with_its_tokens() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let id = recorder.created(&spec("writer", "g1"));
        assert_eq!(read_agent_tasks(&log)[0].state, TaskState::Pending);

        recorder.running(&id);
        let running = &read_agent_tasks(&log)[0];
        assert_eq!(running.state, TaskState::Running);
        assert!(running.started_at_ms.is_some());

        recorder.finished(&id, TaskOutcome::Done { result: "the text".into(), usage: Some(Usage { prompt_tokens: 3, completion_tokens: 4, total_tokens: 7 }) });
        let done = &read_agent_tasks(&log)[0];
        assert_eq!((done.state, done.result.as_deref(), done.usage.map(|u| u.total_tokens)), (TaskState::Done, Some("the text"), Some(7)));
        assert_eq!((done.owner.as_deref(), done.assignee.as_str(), done.model.as_deref(), done.group.as_str()), (Some("chief"), "writer", Some("fast"), "g1"));
        assert!(done.finished_at_ms.is_some());
    }

    #[test]
    fn a_failed_and_a_cancelled_task_say_why() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let failed = recorder.created(&spec("a", "g"));
        recorder.finished(&failed, TaskOutcome::Failed { error: "model exploded".into() });
        let cancelled = recorder.created(&spec("b", "g"));
        recorder.finished(&cancelled, TaskOutcome::Cancelled);

        let tasks = read_agent_tasks(&log);
        let by = |id: &str| tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!((by(&failed).state, by(&failed).error.as_deref()), (TaskState::Failed, Some("model exploded")));
        assert_eq!(by(&cancelled).state, TaskState::Cancelled);
        assert!(by(&cancelled).error.as_deref().unwrap().contains("turn ended"));
    }

    #[test]
    fn ids_are_unique_and_the_newest_task_comes_first() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let ids: Vec<String> = (0..5).map(|n| recorder.created(&spec(&format!("a{n}"), "g"))).collect();
        assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 5);
        let tasks = read_agent_tasks(&log);
        assert_eq!(tasks.len(), 5);
        assert_eq!(tasks[0].assignee, "a4", "newest first");
    }

    #[test]
    fn a_line_that_does_not_parse_and_a_missing_file_are_not_an_error() {
        let log = temp_log();
        assert!(read_agent_tasks(&log).is_empty());
        let recorder = FileTaskRecorder::new(&log);
        recorder.created(&spec("a", "g"));
        let mut text = std::fs::read_to_string(&log).unwrap();
        text.push_str("{ not json\n\n{\"e\":\"running\",\"id\":\"ghost\",\"at\":1}\n");
        std::fs::write(&log, text).unwrap();
        assert_eq!(read_agent_tasks(&log).len(), 1, "an event about a task nobody created is ignored too");
    }

    #[test]
    fn only_the_newest_tasks_are_kept() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        for n in 0..(MAX_TASKS + 20) {
            recorder.created(&spec(&format!("a{n}"), "g"));
        }
        let tasks = read_agent_tasks(&log);
        assert_eq!(tasks.len(), MAX_TASKS);
        assert_eq!(tasks[0].assignee, format!("a{}", MAX_TASKS + 19));
    }

    #[test]
    fn a_task_last_seen_long_ago_is_shown_as_cancelled_not_running_forever() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let id = recorder.created(&spec("a", "g"));
        recorder.running(&id);
        let finished = recorder.created(&spec("b", "g"));
        recorder.finished(&finished, TaskOutcome::Done { result: "ok".into(), usage: None });

        let much_later = now_ms() + STALE_AFTER_MS + 1000;
        let tasks = read_agent_tasks_at(&log, much_later);
        let by = |id: &str| tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(by(&id).state, TaskState::Cancelled);
        assert!(by(&id).error.as_deref().unwrap().contains("interrupted"));
        assert_eq!(by(&finished).state, TaskState::Done, "a finished task stays as it was");
        assert_eq!(read_agent_tasks(&log).iter().find(|t| t.id == id).unwrap().state, TaskState::Running, "and a recent one is still running");
    }

    #[test]
    fn long_text_is_clipped() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let id = recorder.created(&TaskSpec { objective: "o".repeat(5000), ..spec("a", "g") });
        recorder.finished(&id, TaskOutcome::Done { result: "r".repeat(9000), usage: None });
        let task = &read_agent_tasks(&log)[0];
        assert_eq!(task.objective.chars().count(), MAX_OBJECTIVE_CHARS + 1);
        assert_eq!(task.result.as_deref().unwrap().chars().count(), MAX_RESULT_CHARS + 1);
    }

    #[test]
    fn compacting_keeps_each_task_as_it_stood() {
        let log = temp_log();
        let recorder = FileTaskRecorder::new(&log);
        let id = recorder.created(&spec("a", "g"));
        recorder.running(&id);
        recorder.finished(&id, TaskOutcome::Done { result: "ok".into(), usage: None });
        let other = recorder.created(&spec("b", "g"));
        let before = read_agent_tasks(&log);

        {
            let _held = recorder.write.lock().unwrap();
            recorder.compact();
        }
        let after = read_agent_tasks(&log);
        assert_eq!(before, after);
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2, "one snapshot per task");
        // And it keeps working after a compaction.
        recorder.running(&other);
        assert_eq!(read_agent_tasks(&log).iter().find(|t| t.id == other).unwrap().state, TaskState::Running);
    }

    #[test]
    fn a_log_that_cannot_be_written_does_not_stop_anything() {
        let recorder = FileTaskRecorder::new(std::env::temp_dir().join("warden-no-such-dir-\0bad").join("x.jsonl"));
        let id = recorder.created(&spec("a", "g"));
        recorder.running(&id);
        recorder.finished(&id, TaskOutcome::Cancelled);
    }
}
