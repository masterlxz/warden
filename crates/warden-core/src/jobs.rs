use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::{watch, Semaphore};
use tokio::task::JoinHandle;

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

/// A row of `JobBoard::list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSummary {
    pub id: String,
    pub label: String,
    pub state: JobState,
}

struct Job {
    id: String,
    label: String,
    state: watch::Receiver<JobState>,
    handle: JoinHandle<()>,
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
}

impl JobBoard {
    /// `max_parallel` is clamped to at least 1, so a job always eventually gets to run.
    pub fn new(max_parallel: usize) -> Arc<Self> {
        Arc::new(Self { slots: Arc::new(Semaphore::new(max_parallel.max(1))), jobs: Mutex::new(Vec::new()) })
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
        jobs.push(Job { id: id.clone(), label, state: state_rx, handle });
        id
    }

    pub fn list(&self) -> Vec<JobSummary> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter().map(|j| JobSummary { id: j.id.clone(), label: j.label.clone(), state: j.state.borrow().clone() }).collect()
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
