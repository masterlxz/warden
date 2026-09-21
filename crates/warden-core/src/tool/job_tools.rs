use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::jobs::{JobBoard, JobState};
use crate::tool::{Tool, ToolSpec};

/// The `background` argument the delegation tools (`delegate_task`, `delegate_to_agent`) accept once
/// a turn has a `JobBoard` — added to their spec only then, so nothing advertises it otherwise.
pub fn background_property() -> Value {
    json!({
        "type": "boolean",
        "description": "Set true to start the task as a background job: this returns a job_id at once \
            and the sub-agent works while you carry on, several at a time. Collect each result with \
            the 'jobs' tool (action 'result') BEFORE you answer — jobs still unfinished when your \
            answer is given are cancelled. Leave it out to wait for the result here, as usual."
    })
}

/// Whether a delegation call asked for a background job.
pub fn wants_background(args: &Value) -> bool {
    args.get("background").and_then(Value::as_bool).unwrap_or(false)
}

/// Queues `work` on `board` and returns what the model gets back in place of the result.
pub fn start_background<F>(board: &JobBoard, label: String, work: F) -> Value
where
    F: Future<Output = anyhow::Result<String>> + Send + 'static,
{
    let job_id = board.spawn(label, work);
    json!({
        "job_id": job_id,
        "status": "started",
        "next": "Carry on; read the outcome later with the 'jobs' tool (action 'result', this job_id)."
    })
}

/// A short label for a listing: who runs it and the start of the task.
pub fn job_label(who: &str, task: &str) -> String {
    let task: String = task.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = task.chars().take(60).collect();
    let ellipsis = if task.chars().count() > 60 { "…" } else { "" };
    format!("{who}: {short}{ellipsis}")
}

/// Lists the turn's background jobs and reads their results (P46, job queue). Registered once at
/// startup, unbound; `Orchestrator::with_turn_jobs` binds a copy to each turn's `JobBoard`. Unbound
/// it is not offered to the model (`is_available`), because there is nothing it could read.
pub struct JobsTool {
    board: Option<Arc<JobBoard>>,
}

impl JobsTool {
    pub fn new() -> Self {
        Self { board: None }
    }
}

impl Default for JobsTool {
    fn default() -> Self {
        Self::new()
    }
}

fn outcome(id: &str, state: &JobState) -> Value {
    match state {
        JobState::Done(text) => json!({ "job_id": id, "state": "done", "result": text }),
        JobState::Failed(error) => json!({ "job_id": id, "state": "failed", "error": error }),
        other => json!({ "job_id": id, "state": other.name() }),
    }
}

#[async_trait]
impl Tool for JobsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "jobs".to_string(),
            description: "Check the background jobs you started with 'background: true'. Action 'list' shows \
                every job and whether it is queued, running, done or failed. Action 'result' returns one job's \
                outcome and, by default, waits until it has finished. Read every job you started before you \
                give your final answer: a job still unfinished then is cancelled and its work is lost."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "result"] },
                    "job_id": { "type": "string", "description": "Which job (from 'list' or from the call that started it). Required for 'result'." },
                    "wait": {
                        "type": "boolean",
                        "description": "For 'result': wait for the job to finish (default true). With false, a job still running just reports its state."
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn is_available(&self) -> bool {
        self.board.is_some()
    }

    fn with_jobs(&self, board: &Arc<JobBoard>) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self { board: Some(board.clone()) }))
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let board = self.board.as_ref().ok_or_else(|| anyhow::anyhow!("there are no background jobs in this turn"))?;
        let action = args.get("action").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'action' argument"))?;
        match action {
            "list" => {
                let jobs: Vec<Value> =
                    board.list().iter().map(|j| json!({ "job_id": j.id, "label": j.label, "state": j.state.name() })).collect();
                Ok(json!({ "jobs": jobs }))
            }
            "result" => {
                let id = args.get("job_id").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'job_id' argument"))?;
                let unknown = || {
                    let known = board.list().iter().map(|j| j.id.clone()).collect::<Vec<_>>().join(", ");
                    anyhow::anyhow!("unknown job_id '{id}' — this turn's jobs: {}", if known.is_empty() { "(none)".to_string() } else { known })
                };
                let wait = args.get("wait").and_then(Value::as_bool).unwrap_or(true);
                let state = if wait { board.wait(id).await } else { board.state(id) };
                Ok(outcome(id, &state.ok_or_else(unknown)?))
            }
            other => anyhow::bail!("unknown action '{other}' — use 'list' or 'result'"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn bound() -> (Arc<dyn Tool>, Arc<JobBoard>) {
        let board = JobBoard::new(2);
        (JobsTool::new().with_jobs(&board).unwrap(), board)
    }

    #[test]
    fn unbound_it_is_hidden_from_the_model() {
        assert!(!JobsTool::new().is_available());
        assert!(bound().0.is_available());
    }

    #[tokio::test]
    async fn result_waits_for_a_running_job_and_returns_its_answer() {
        let (tool, board) = bound();
        let id = board.spawn("slow".into(), async {
            tokio::time::sleep(Duration::from_millis(40)).await;
            Ok("finished".to_string())
        });

        let out = tool.call(json!({ "action": "result", "job_id": id })).await.unwrap();

        assert_eq!(out, json!({ "job_id": "job-1", "state": "done", "result": "finished" }));
    }

    #[tokio::test]
    async fn result_without_waiting_reports_the_state_of_an_unfinished_job() {
        let (tool, board) = bound();
        let id = board.spawn("hung".into(), async {
            std::future::pending::<()>().await;
            Ok(String::new())
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let out = tool.call(json!({ "action": "result", "job_id": id, "wait": false })).await.unwrap();

        assert_eq!(out, json!({ "job_id": "job-1", "state": "running" }));
        board.abort_unfinished();
    }

    #[tokio::test]
    async fn a_failed_job_comes_back_as_an_error_field_not_a_tool_error() {
        let (tool, board) = bound();
        let id = board.spawn("bad".into(), async { anyhow::bail!("limit of 3 model calls reached") });

        let out = tool.call(json!({ "action": "result", "job_id": id })).await.unwrap();

        assert_eq!(out["state"], "failed");
        assert!(out["error"].as_str().unwrap().contains("limit of 3"));
    }

    #[tokio::test]
    async fn list_shows_every_job_with_its_state() {
        let (tool, board) = bound();
        let id = board.spawn("writer: draft".into(), async { Ok("x".to_string()) });
        board.wait(&id).await;

        let out = tool.call(json!({ "action": "list" })).await.unwrap();

        assert_eq!(out, json!({ "jobs": [{ "job_id": "job-1", "label": "writer: draft", "state": "done" }] }));
    }

    #[tokio::test]
    async fn an_unknown_job_id_names_the_ones_that_exist() {
        let (tool, board) = bound();
        board.spawn("a".into(), async { Ok(String::new()) });

        let err = tool.call(json!({ "action": "result", "job_id": "job-7" })).await.unwrap_err().to_string();

        assert!(err.contains("job-7") && err.contains("job-1"), "{err}");
    }

    #[tokio::test]
    async fn bad_arguments_are_refused_clearly() {
        let (tool, _) = bound();
        assert!(tool.call(json!({})).await.unwrap_err().to_string().contains("action"));
        assert!(tool.call(json!({ "action": "result" })).await.unwrap_err().to_string().contains("job_id"));
        assert!(tool.call(json!({ "action": "cancel" })).await.unwrap_err().to_string().contains("unknown action"));
    }

    #[test]
    fn labels_are_short_and_single_line() {
        assert_eq!(job_label("poet", "write\n  a   poem"), "poet: write a poem");
        let long = job_label("poet", &"x".repeat(100));
        assert_eq!(long.chars().count(), "poet: ".len() + 60 + 1);
        assert!(long.ends_with('…'));
    }
}
