use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::{watch, Semaphore};
use tokio::task::JoinHandle;

use crate::model::Usage;

/// Where a background job is (P46, job queue). `Queued` = waiting for a free slot under the
/// concurrency limit; the other three are self-explanatory. `Done`/`Failed` carry what the job
/// produced, so a result can be read any number of times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    Done(String),
    Failed(String),
}

impl JobState {
    pub fn is_finished(&self) -> bool {
        matches!(self, JobState::Done(_) | JobState::Failed(_))
    }

    /// One word for a listing.
    pub fn name(&self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done(_) => "done",
            JobState::Failed(_) => "failed",
        }
    }
}

/// What a recorded task is about (P123): who does it, what for, and on which model. The rest of what a record carries
/// (the turn's group, the agent that delegated, the channel) comes from the turn itself (`TaskContext`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDraft {
    /// The agent that does the work, or the name the delegating agent gave a temporary helper.
    pub assignee: String,
    pub objective: String,
    /// The provider or combo chosen for this task, `None` for the assignee's own.
    pub model: Option<String>,
}

/// A task as the recorder is told about it (P123): a `TaskDraft` placed in the turn that created it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec {
    /// Shared by every task one turn starts, so a screen can show them together with a progress.
    pub group: String,
    /// The agent that delegated (the one the turn speaks as), if there was one.
    pub owner: Option<String>,
    pub assignee: String,
    pub objective: String,
    pub model: Option<String>,
    pub channel: String,
}

/// How a recorded task ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskOutcome {
    Done { result: String, usage: Option<Usage> },
    Failed { error: String },
    /// The turn ended (or was cancelled) before it finished.
    Cancelled,
}

/// Keeps the tasks the background jobs run as records that outlive the turn (P123). Implemented by the host
/// (`warden-bootstrap`'s file log); `created` hands back the record's id, and every later call names it.
pub trait TaskRecorder: Send + Sync {
    fn created(&self, task: &TaskSpec) -> String;
    fn running(&self, id: &str);
    fn finished(&self, id: &str, outcome: TaskOutcome);
}

/// What a turn tells its board about itself, to place the tasks it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskContext {
    pub group: String,
    pub owner: Option<String>,
    pub channel: String,
}

/// What a recorded job adds to its row in `JobBoard::list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobTask {
    pub task_id: String,
    pub assignee: String,
    pub model: Option<String>,
    /// Known once the job has finished and the model reported usage.
    pub total_tokens: Option<u64>,
}

/// A row of `JobBoard::list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSummary {
    pub id: String,
    pub label: String,
    pub state: JobState,
    /// Only for a job that is recorded as a task.
    pub task: Option<JobTask>,
}

struct Job {
    id: String,
    label: String,
    state: watch::Receiver<JobState>,
    handle: JoinHandle<()>,
    task: Option<RecordedTask>,
}

struct RecordedTask {
    task_id: String,
    assignee: String,
    model: Option<String>,
    usage: Arc<Mutex<Option<Usage>>>,
}

/// Marks a recorded task cancelled when the future that owns it is dropped before it finished — which is what
/// `abort_unfinished` does to a job — and does nothing once the outcome has been written.
struct CancelOnDrop {
    recorder: Arc<dyn TaskRecorder>,
    id: String,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.recorder.finished(&self.id, TaskOutcome::Cancelled);
        }
    }
}

/// The background jobs of one turn (P46): sub-agent tasks a "chief" started without waiting for
/// them, so several run at once while it keeps working, and it collects the results later. At most
/// `max_parallel` run at the same time; the rest wait in `Queued`, in the order they were started.
///
/// Lives exactly as long as the turn that created it — see `JobsGuard`, which aborts whatever has
/// not finished when the turn ends (or is cancelled). Nothing here is persisted.
pub struct JobBoard {
    slots: Arc<Semaphore>,
    jobs: Mutex<Vec<Job>>,
    /// Where the tasks of this turn are recorded (P123), and the turn they belong to. `None`: jobs stay in memory.
    recording: Option<(Arc<dyn TaskRecorder>, TaskContext)>,
}

impl JobBoard {
    /// `max_parallel` is clamped to at least 1, so a job always eventually gets to run.
    pub fn new(max_parallel: usize) -> Arc<Self> {
        Arc::new(Self { slots: Arc::new(Semaphore::new(max_parallel.max(1))), jobs: Mutex::new(Vec::new()), recording: None })
    }

    /// Like `new`, with every task started through `spawn_task` recorded by `recorder` as part of `context`'s turn.
    pub fn recording(max_parallel: usize, recorder: Arc<dyn TaskRecorder>, context: TaskContext) -> Arc<Self> {
        Arc::new(Self { slots: Arc::new(Semaphore::new(max_parallel.max(1))), jobs: Mutex::new(Vec::new()), recording: Some((recorder, context)) })
    }

    /// Like `spawn`, for work that is a task (P123): when the board records, the task is written as pending now, running
    /// when it gets a slot, and finished (done with its tokens, failed, or cancelled with the turn) when it ends. Without a
    /// recorder it is a plain job, and the usage is dropped.
    pub fn spawn_task<F>(&self, label: String, draft: TaskDraft, work: F) -> String
    where
        F: Future<Output = anyhow::Result<(String, Option<Usage>)>> + Send + 'static,
    {
        let Some((recorder, context)) = &self.recording else {
            return self.spawn(label, async move { work.await.map(|(text, _)| text) });
        };
        let task_id = recorder.created(&TaskSpec {
            group: context.group.clone(),
            owner: context.owner.clone(),
            assignee: draft.assignee.clone(),
            objective: draft.objective.clone(),
            model: draft.model.clone(),
            channel: context.channel.clone(),
        });
        let usage_slot = Arc::new(Mutex::new(None));
        let (state_tx, state_rx) = watch::channel(JobState::Queued);
        let slots = self.slots.clone();
        let (recorder, id, usage) = (recorder.clone(), task_id.clone(), usage_slot.clone());
        let handle = tokio::spawn(async move {
            let mut cancel = CancelOnDrop { recorder: recorder.clone(), id: id.clone(), armed: true };
            let Ok(_slot) = slots.acquire_owned().await else {
                cancel.armed = false;
                recorder.finished(&id, TaskOutcome::Failed { error: "the job queue was closed".to_string() });
                state_tx.send_replace(JobState::Failed("the job queue was closed".to_string()));
                return;
            };
            state_tx.send_replace(JobState::Running);
            recorder.running(&id);
            let outcome = work.await;
            cancel.armed = false;
            match outcome {
                Ok((text, used)) => {
                    *usage.lock().unwrap_or_else(|e| e.into_inner()) = used;
                    recorder.finished(&id, TaskOutcome::Done { result: text.clone(), usage: used });
                    state_tx.send_replace(JobState::Done(text));
                }
                Err(err) => {
                    let error = format!("{err:#}");
                    recorder.finished(&id, TaskOutcome::Failed { error: error.clone() });
                    state_tx.send_replace(JobState::Failed(error));
                }
            }
        });

        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let id = format!("job-{}", jobs.len() + 1);
        jobs.push(Job { id: id.clone(), label, state: state_rx, handle, task: Some(RecordedTask { task_id, assignee: draft.assignee, model: draft.model, usage: usage_slot }) });
        id
    }

    /// Queues `work` and returns its id (`job-1`, `job-2`, ...) right away, without waiting. `label`
    /// is only for listings — typically which agent runs it and a short piece of the task.
    pub fn spawn<F>(&self, label: String, work: F) -> String
    where
        F: Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        let (state_tx, state_rx) = watch::channel(JobState::Queued);
        let slots = self.slots.clone();
        let handle = tokio::spawn(async move {
            // Held for the whole run; released when this task ends, however it ends.
            let Ok(_slot) = slots.acquire_owned().await else {
                state_tx.send_replace(JobState::Failed("the job queue was closed".to_string()));
                return;
            };
            state_tx.send_replace(JobState::Running);
            let outcome = work.await;
            state_tx.send_replace(match outcome {
                Ok(text) => JobState::Done(text),
                Err(err) => JobState::Failed(format!("{err:#}")),
            });
        });

        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let id = format!("job-{}", jobs.len() + 1);
        jobs.push(Job { id: id.clone(), label, state: state_rx, handle, task: None });
        id
    }

    pub fn list(&self) -> Vec<JobSummary> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter()
            .map(|j| JobSummary {
                id: j.id.clone(),
                label: j.label.clone(),
                state: j.state.borrow().clone(),
                task: j.task.as_ref().map(|t| JobTask {
                    task_id: t.task_id.clone(),
                    assignee: t.assignee.clone(),
                    model: t.model.clone(),
                    total_tokens: t.usage.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|u| u64::from(u.total_tokens)),
                }),
            })
            .collect()
    }

    /// Where `id` is right now, or `None` for an id that was never handed out.
    pub fn state(&self, id: &str) -> Option<JobState> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter().find(|j| j.id == id).map(|j| j.state.borrow().clone())
    }

    /// Waits until `id` has finished and returns how, or `None` for an unknown id. A job whose task
    /// died without reporting (a panic) comes back as `Failed`, never as a hang.
    pub async fn wait(&self, id: &str) -> Option<JobState> {
        let mut state = {
            let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.iter().find(|j| j.id == id)?.state.clone()
        };
        let finished = state.wait_for(JobState::is_finished).await.map(|s| s.clone());
        Some(finished.unwrap_or_else(|_| JobState::Failed("the job stopped without a result (it panicked)".to_string())))
    }

    /// Stops every job that has not finished. Finished ones keep their result.
    pub fn abort_unfinished(&self) {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        for job in jobs.iter() {
            if !job.state.borrow().is_finished() {
                job.handle.abort();
            }
        }
    }
}

/// Held by whoever owns the turn: dropping it — the turn ended, failed, or its future was dropped
/// because the user cancelled — aborts the jobs still running, so nothing outlives the turn.
pub struct JobsGuard(pub Arc<JobBoard>);

impl Drop for JobsGuard {
    fn drop(&mut self) {
        self.0.abort_unfinished();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::sync::Notify;

    use super::*;

    #[tokio::test]
    async fn runs_a_job_and_hands_back_its_result() {
        let board = JobBoard::new(2);
        let id = board.spawn("echo".into(), async { Ok("hello".to_string()) });

        assert_eq!(id, "job-1");
        assert_eq!(board.wait(&id).await, Some(JobState::Done("hello".to_string())));
        // A finished result can be read again.
        assert_eq!(board.state(&id), Some(JobState::Done("hello".to_string())));
    }

    #[tokio::test]
    async fn a_failing_job_reports_the_error_instead_of_a_result() {
        let board = JobBoard::new(2);
        let id = board.spawn("bad".into(), async { anyhow::bail!("model exploded") });

        match board.wait(&id).await {
            Some(JobState::Failed(message)) => assert!(message.contains("model exploded"), "{message}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unknown_id_is_none_not_a_hang() {
        let board = JobBoard::new(1);
        assert_eq!(board.wait("job-9").await, None);
        assert_eq!(board.state("job-9"), None);
    }

    #[tokio::test]
    async fn never_runs_more_jobs_at_once_than_the_limit() {
        let board = JobBoard::new(2);
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let ids: Vec<String> = (0..5)
            .map(|n| {
                let (active, peak) = (active.clone(), peak.clone());
                board.spawn(format!("job {n}"), async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok("ok".to_string())
                })
            })
            .collect();

        for id in &ids {
            assert_eq!(board.wait(id).await, Some(JobState::Done("ok".to_string())));
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_job_past_the_limit_waits_queued_until_a_slot_frees() {
        let board = JobBoard::new(1);
        let release = Arc::new(Notify::new());
        let gate = release.clone();
        let first = board.spawn("first".into(), async move {
            gate.notified().await;
            Ok("one".to_string())
        });
        let second = board.spawn("second".into(), async { Ok("two".to_string()) });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(board.state(&first), Some(JobState::Running));
        assert_eq!(board.state(&second), Some(JobState::Queued));

        release.notify_one();
        assert_eq!(board.wait(&second).await, Some(JobState::Done("two".to_string())));
    }

    #[tokio::test]
    async fn a_zero_limit_still_lets_jobs_run_one_at_a_time() {
        let board = JobBoard::new(0);
        let id = board.spawn("solo".into(), async { Ok("ran".to_string()) });
        assert_eq!(board.wait(&id).await, Some(JobState::Done("ran".to_string())));
    }

    #[tokio::test]
    async fn list_reports_every_job_in_start_order() {
        let board = JobBoard::new(2);
        board.spawn("a".into(), async { Ok("1".to_string()) });
        let b = board.spawn("b".into(), async { Ok("2".to_string()) });
        board.wait(&b).await;

        let listed = board.list();
        assert_eq!(listed.iter().map(|j| (j.id.as_str(), j.label.as_str())).collect::<Vec<_>>(), [("job-1", "a"), ("job-2", "b")]);
    }

    /// Flips a flag when the future holding it is dropped — i.e. when the task was aborted.
    struct DropFlag(Arc<AtomicUsize>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn dropping_the_guard_aborts_unfinished_jobs_but_keeps_finished_results() {
        let board = JobBoard::new(2);
        let aborted = Arc::new(AtomicUsize::new(0));
        let flag = DropFlag(aborted.clone());
        let done = board.spawn("quick".into(), async { Ok("kept".to_string()) });
        board.wait(&done).await;
        let hung = board.spawn("hung".into(), async move {
            let _flag = flag;
            std::future::pending::<()>().await;
            Ok(String::new())
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(board.state(&hung), Some(JobState::Running));

        drop(JobsGuard(board.clone()));
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(aborted.load(Ordering::SeqCst), 1, "the hung job's future should have been dropped");
        assert_eq!(board.state(&done), Some(JobState::Done("kept".to_string())));
    }

    /// Writes down what a recorder is told, as short lines.
    struct Log(Mutex<Vec<String>>);

    impl Log {
        fn lines(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    impl TaskRecorder for Log {
        fn created(&self, task: &TaskSpec) -> String {
            let mut lines = self.0.lock().unwrap();
            let id = format!("t{}", lines.iter().filter(|l| l.starts_with("created")).count() + 1);
            lines.push(format!("created {id} {} model={} group={} owner={:?}", task.assignee, task.model.as_deref().unwrap_or("-"), task.group, task.owner));
            id
        }

        fn running(&self, id: &str) {
            self.0.lock().unwrap().push(format!("running {id}"));
        }

        fn finished(&self, id: &str, outcome: TaskOutcome) {
            self.0.lock().unwrap().push(match outcome {
                TaskOutcome::Done { result, usage } => format!("done {id} {result} tokens={}", usage.map_or(0, |u| u.total_tokens)),
                TaskOutcome::Failed { error } => format!("failed {id} {error}"),
                TaskOutcome::Cancelled => format!("cancelled {id}"),
            });
        }
    }

    fn recording_board(max: usize) -> (Arc<JobBoard>, Arc<Log>) {
        let log = Arc::new(Log(Mutex::new(Vec::new())));
        let context = TaskContext { group: "g1".into(), owner: Some("chief".into()), channel: "desktop".into() };
        (JobBoard::recording(max, log.clone(), context), log)
    }

    fn draft(assignee: &str) -> TaskDraft {
        TaskDraft { assignee: assignee.into(), objective: "do it".into(), model: Some("fast".into()) }
    }

    #[tokio::test]
    async fn a_task_is_recorded_as_pending_then_running_then_done_with_its_tokens() {
        let (board, log) = recording_board(2);
        let id = board.spawn_task("writer: draft".into(), draft("writer"), async { Ok(("the text".to_string(), Some(Usage { prompt_tokens: 3, completion_tokens: 4, total_tokens: 7 }))) });

        assert_eq!(board.wait(&id).await, Some(JobState::Done("the text".to_string())));
        assert_eq!(
            log.lines(),
            ["created t1 writer model=fast group=g1 owner=Some(\"chief\")", "running t1", "done t1 the text tokens=7"]
        );
        let listed = board.list();
        let task = listed[0].task.as_ref().unwrap();
        assert_eq!((task.task_id.as_str(), task.assignee.as_str(), task.model.as_deref(), task.total_tokens), ("t1", "writer", Some("fast"), Some(7)));
    }

    #[tokio::test]
    async fn a_failing_task_is_recorded_as_failed() {
        let (board, log) = recording_board(2);
        let id = board.spawn_task("bad".into(), draft("bad"), async { anyhow::bail!("model exploded") });
        board.wait(&id).await;
        let lines = log.lines();
        assert_eq!(lines[1], "running t1");
        assert!(lines[2].starts_with("failed t1") && lines[2].contains("model exploded"), "{lines:?}");
    }

    #[tokio::test]
    async fn a_task_the_turn_aborts_is_recorded_as_cancelled_and_one_still_queued_too() {
        let (board, log) = recording_board(1);
        board.spawn_task("first".into(), draft("a"), async {
            std::future::pending::<()>().await;
            Ok((String::new(), None))
        });
        board.spawn_task("second".into(), draft("b"), async { Ok(("never".to_string(), None)) });
        tokio::time::sleep(Duration::from_millis(40)).await;
        // The second is waiting for the one slot: pending, not running.
        assert!(log.lines().iter().any(|l| l == "running t1") && !log.lines().iter().any(|l| l == "running t2"), "{:?}", log.lines());

        drop(JobsGuard(board.clone()));
        tokio::time::sleep(Duration::from_millis(60)).await;
        let lines = log.lines();
        assert!(lines.contains(&"cancelled t1".to_string()) && lines.contains(&"cancelled t2".to_string()), "{lines:?}");
    }

    #[tokio::test]
    async fn a_finished_task_is_not_marked_cancelled_when_the_turn_ends() {
        let (board, log) = recording_board(1);
        let id = board.spawn_task("quick".into(), draft("a"), async { Ok(("ok".to_string(), None)) });
        board.wait(&id).await;
        drop(JobsGuard(board.clone()));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!log.lines().iter().any(|l| l.starts_with("cancelled")), "{:?}", log.lines());
    }

    #[tokio::test]
    async fn without_a_recorder_a_task_is_a_plain_job() {
        let board = JobBoard::new(1);
        let id = board.spawn_task("plain".into(), draft("a"), async { Ok(("fine".to_string(), None)) });
        assert_eq!(board.wait(&id).await, Some(JobState::Done("fine".to_string())));
        assert!(board.list()[0].task.is_none());
    }

    #[tokio::test]
    async fn waiting_on_an_aborted_job_reports_failure_instead_of_hanging() {
        let board = JobBoard::new(1);
        let hung = board.spawn("hung".into(), async {
            std::future::pending::<()>().await;
            Ok(String::new())
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        board.abort_unfinished();

        assert!(matches!(board.wait(&hung).await, Some(JobState::Failed(_))));
    }
}
