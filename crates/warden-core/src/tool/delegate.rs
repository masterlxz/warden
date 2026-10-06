use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::budget::TurnBudget;
use crate::jobs::{JobBoard, TaskDraft};
use crate::model::ModelProvider;
use crate::orchestrator::Orchestrator;
use crate::tool::job_tools::{background_property, job_label, start_task_with, wants_background};
use crate::tool::{Tool, ToolSpec};

/// Delegates a scoped, self-contained task to a fresh sub-agent — its own `Orchestrator`
/// instance (same model, same vault, a caller-chosen subset of tools). Originally the
/// "invocação leve" v1 mechanism from ARCHITECTURE.md's "Sub-agentes" section (single level,
/// no recursion); P46 extended it to support **bounded recursive delegation** — the
/// `orchestrator` passed to `new` may itself have its own `DelegateTool` registered, wrapping a
/// further sub-orchestrator, and so on. The stopping criterion isn't a runtime depth check here:
/// it's structural — whoever builds the chain (`warden-bootstrap::build_delegating_orchestrator`)
/// stops registering a `DelegateTool` past a fixed depth, so the terminal orchestrator simply
/// never advertises `delegate_task` in its tool specs and a model calling it has nothing further
/// to delegate to. Still no job queue, no persistence, no cost control, no tool isolation beyond
/// the caller-chosen subset — those remain out of scope (see `PENDING.md` P46/P60).
pub struct DelegateTool {
    orchestrator: Orchestrator,
    /// The turn's background jobs, once bound (`with_jobs`). Only then does the tool accept
    /// `background: true` and say so in its spec.
    jobs: Option<Arc<JobBoard>>,
    /// The models the caller may pick for a task (P123), when the host offers any: the `model` argument.
    models: Option<ModelChoices>,
}

/// The models a delegating agent may choose between, one per task (P123): the ids of the configured providers and combos,
/// and how to turn one into a model. Built by the host, which knows the config; `warden-core` only calls it.
#[derive(Clone)]
pub struct ModelChoices {
    pub ids: Vec<String>,
    /// What a choice is for, by id (the named policies, "fast", "reasoning"...): shown to the agent next to the ids.
    pub hints: Vec<(String, String)>,
    pub resolve: ModelResolver,
}

/// Turns the id of a configured provider or combo into the model that runs a task.
pub type ModelResolver = Arc<dyn Fn(&str) -> anyhow::Result<Arc<dyn ModelProvider>> + Send + Sync>;

/// The `model` argument of a delegation: which provider or combo does this task.
pub fn model_property(choices: &ModelChoices) -> Value {
    let mut description = "Which model does this task. Leave it out to use the default one. Pick a faster or cheaper model for \
        simple work and a stronger one for hard reasoning or code: you decide task by task."
        .to_string();
    for (id, hint) in &choices.hints {
        description.push_str(&format!("\n- {id}: {hint}"));
    }
    json!({ "type": "string", "enum": choices.ids, "description": description })
}

/// The model a call asked for, or `None` for the default. An id that isn't one of the choices is refused, naming the ones that are.
pub fn pick_model(choices: &Option<ModelChoices>, args: &Value) -> anyhow::Result<Option<(String, Arc<dyn ModelProvider>)>> {
    let Some(wanted) = args.get("model").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    let Some(choices) = choices else {
        anyhow::bail!("this agent can't choose a model for a task — leave out 'model'");
    };
    if !choices.ids.iter().any(|id| id == wanted) {
        anyhow::bail!("unknown model '{wanted}' — choose one of: {}", choices.ids.join(", "));
    }
    Ok(Some((wanted.to_string(), (choices.resolve)(wanted)?)))
}

/// What the sub-agent of a delegation is called in a listing: the name the caller gave a temporary helper, if any.
fn helper_name(args: &Value) -> String {
    args.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| "helper".to_string(), |n| n.chars().take(60).collect())
}

impl DelegateTool {
    pub fn new(orchestrator: Orchestrator) -> Self {
        Self { orchestrator, jobs: None, models: None }
    }

    /// Lets the caller pick a model for each task (P123).
    pub fn with_models(mut self, models: ModelChoices) -> Self {
        self.models = Some(models);
        self
    }

    fn rebuilt(&self, orchestrator: Orchestrator, jobs: Option<Arc<JobBoard>>) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self { orchestrator, jobs, models: self.models.clone() }))
    }
}

#[async_trait]
impl Tool for DelegateTool {
    fn spec(&self) -> ToolSpec {
        let mut properties = json!({
            "task": {
                "type": "string",
                "description": "A complete, self-contained description of the task for \
                    the sub-agent to perform. Include everything it needs to know, since \
                    it starts with no memory of the current conversation."
            }
        });
        if self.jobs.is_some() {
            properties["background"] = background_property();
            properties["name"] = json!({
                "type": "string",
                "description": "A short name for this temporary helper (e.g. 'schema reviewer'), shown with the task in the progress view."
            });
        }
        if let Some(models) = &self.models {
            properties["model"] = model_property(models);
        }
        ToolSpec {
            name: "delegate_task".to_string(),
            description: "Delegate a scoped, self-contained task to a fresh sub-agent that runs \
                independently and reports back only its final answer. Use this to offload a \
                bounded chunk of work (e.g. summarizing a file, researching a topic, drafting a \
                note) so its intermediate tool calls don't clutter the current conversation. \
                IMPORTANT: the sub-agent does NOT see this conversation's history — the 'task' \
                argument must be a fully self-contained description including any facts, file \
                paths, or constraints it needs. The sub-agent cannot ask you follow-up questions \
                and cannot itself delegate further; it runs to completion and returns one result."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": properties,
                "required": ["task"]
            }),
        }
    }

    fn with_budget(&self, budget: &Arc<TurnBudget>) -> Option<Arc<dyn Tool>> {
        self.rebuilt(self.orchestrator.charged_to(budget.clone()), self.jobs.clone())
    }

    fn restricted_to(&self, allowed: &[String]) -> Option<Arc<dyn Tool>> {
        self.rebuilt(self.orchestrator.with_allowed_tools(Some(allowed)), self.jobs.clone())
    }

    fn with_autonomy(&self, level: crate::autonomy::Autonomy, read_only: &[String]) -> Option<Arc<dyn Tool>> {
        self.rebuilt(self.orchestrator.with_autonomy(level, read_only), self.jobs.clone())
    }

    fn with_approval_rules(&self, required: &[crate::autonomy::Category], classifier: Option<&crate::autonomy::Classifier>) -> Option<Arc<dyn Tool>> {
        self.rebuilt(self.orchestrator.with_approval_rules(required, classifier.cloned()), self.jobs.clone())
    }

    fn with_vault(&self, vault: &Arc<crate::memory::Vault>) -> Option<Arc<dyn Tool>> {
        self.rebuilt(self.orchestrator.with_vault(vault.clone()), self.jobs.clone())
    }

    fn with_media_root(&self, root: &std::path::Path) -> Option<Arc<dyn Tool>> {
        self.rebuilt(self.orchestrator.with_media_root(root.to_path_buf()), self.jobs.clone())
    }

    fn with_jobs(&self, board: &Arc<JobBoard>) -> Option<Arc<dyn Tool>> {
        self.rebuilt(self.orchestrator.clone(), Some(board.clone()))
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let task = args
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'task' argument"))?;
        let (orchestrator, model_id) = match pick_model(&self.models, &args)? {
            Some((id, model)) => (self.orchestrator.with_model(model), Some(id)),
            None => (self.orchestrator.clone(), None),
        };

        if let (Some(board), true) = (&self.jobs, wants_background(&args)) {
            let owned_task = task.to_string();
            let name = helper_name(&args);
            let draft = TaskDraft { assignee: name.clone(), objective: task.to_string(), model: model_id };
            return Ok(start_task_with(board, job_label(&name, task), draft, move |link| async move {
                // The turn of a task may start subtasks of its own, recorded under it (P123).
                let orchestrator = match link {
                    Some(link) => orchestrator.with_parent_task(link),
                    None => orchestrator,
                };
                let outcome = orchestrator.handle_message(&[], &owned_task).await?;
                Ok((outcome.content, outcome.usage))
            }));
        }

        // Sub-agent's token usage is not rolled up into the parent conversation's total — Tool::call only returns
        // serde_json::Value, not a MessageOutcome. With a task log (P123) it is recorded on the task instead.
        if let Some(board) = &self.jobs {
            let draft = TaskDraft { assignee: helper_name(&args), objective: task.to_string(), model: model_id };
            let owned_task = task.to_string();
            let text = board
                .run_recorded(draft, async move {
                    let outcome = orchestrator.handle_message(&[], &owned_task).await?;
                    Ok((outcome.content, outcome.usage))
                })
                .await?;
            return Ok(json!({ "result": text }));
        }
        let result = orchestrator.handle_message(&[], task).await?;
        Ok(json!({ "result": result.content }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::memory::Vault;
    use crate::model::{ChatStream, Message, ModelProvider, Response, ToolCall, response_stream};

    struct FixedAnswerModel {
        answer: String,
    }

    #[async_trait]
    impl ModelProvider for FixedAnswerModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response { content: self.answer.clone(), tool_calls: Vec::new(), usage: None }))
        }
    }

    fn temp_vault() -> Arc<Vault> {
        Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-delegate-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))))
    }

    #[tokio::test]
    async fn requires_task_argument() {
        let model = Arc::new(FixedAnswerModel { answer: String::new() });
        let tool = DelegateTool::new(Orchestrator::new(model, temp_vault()));

        let err = tool.call(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("task"));
    }

    #[tokio::test]
    async fn delegates_and_returns_sub_agent_final_answer() {
        let model = Arc::new(FixedAnswerModel { answer: "sub-agent done".to_string() });
        let tool = DelegateTool::new(Orchestrator::new(model, temp_vault()));

        let result = tool.call(json!({ "task": "summarize X" })).await.unwrap();
        assert_eq!(result, json!({ "result": "sub-agent done" }));
    }

    fn two_models() -> ModelChoices {
        ModelChoices {
            ids: vec!["fast".to_string(), "strong".to_string()],
            hints: vec![("strong".to_string(), "hard reasoning and code".to_string())],
            resolve: Arc::new(|id| match id {
                "fast" | "strong" => Ok(Arc::new(FixedAnswerModel { answer: format!("answered by {id}") }) as Arc<dyn ModelProvider>),
                other => anyhow::bail!("no model {other}"),
            }),
        }
    }

    #[tokio::test]
    async fn a_chosen_model_does_the_task_and_the_default_one_otherwise() {
        let default = Arc::new(FixedAnswerModel { answer: "answered by default".to_string() });
        let tool = DelegateTool::new(Orchestrator::new(default, temp_vault())).with_models(two_models());

        assert_eq!(tool.call(json!({ "task": "x" })).await.unwrap()["result"], "answered by default");
        assert_eq!(tool.call(json!({ "task": "x", "model": "strong" })).await.unwrap()["result"], "answered by strong");
        assert_eq!(tool.call(json!({ "task": "x", "model": "  " })).await.unwrap()["result"], "answered by default", "a blank model is no choice");
    }

    #[tokio::test]
    async fn an_unknown_model_is_refused_naming_the_ones_there_are_and_none_is_offered_without_choices() {
        let default = Arc::new(FixedAnswerModel { answer: "d".to_string() });
        let tool = DelegateTool::new(Orchestrator::new(default.clone(), temp_vault())).with_models(two_models());
        let err = tool.call(json!({ "task": "x", "model": "huge" })).await.unwrap_err().to_string();
        assert!(err.contains("unknown model 'huge'") && err.contains("fast, strong"), "{err}");

        let plain = DelegateTool::new(Orchestrator::new(default, temp_vault()));
        assert!(plain.spec().parameters["properties"].get("model").is_none(), "no choices, no argument");
        assert!(plain.call(json!({ "task": "x", "model": "fast" })).await.unwrap_err().to_string().contains("can't choose a model"));
        assert_eq!(tool.spec().parameters["properties"]["model"]["enum"], json!(["fast", "strong"]));
    }

    #[tokio::test]
    async fn the_spec_tells_the_agent_what_each_named_choice_is_for() {
        let default = Arc::new(FixedAnswerModel { answer: "d".to_string() });
        let tool = DelegateTool::new(Orchestrator::new(default, temp_vault())).with_models(two_models());
        let description = tool.spec().parameters["properties"]["model"]["description"].as_str().unwrap().to_string();
        assert!(description.contains("- strong: hard reasoning and code") && !description.contains("- fast:"), "{description}");
    }

    #[tokio::test]
    async fn a_delegation_waited_for_is_recorded_with_its_model_when_the_turn_keeps_a_task_log() {
        #[derive(Default)]
        struct Lines(std::sync::Mutex<Vec<String>>);
        impl crate::jobs::TaskRecorder for Lines {
            fn created(&self, task: &crate::jobs::TaskSpec) -> String {
                self.0.lock().unwrap().push(format!("created {} model={}", task.assignee, task.model.as_deref().unwrap_or("-")));
                "t1".to_string()
            }
            fn running(&self, _id: &str) {}
            fn finished(&self, _id: &str, outcome: crate::jobs::TaskOutcome) {
                self.0.lock().unwrap().push(format!("finished {}", matches!(outcome, crate::jobs::TaskOutcome::Done { .. })));
            }
        }
        let log = Arc::new(Lines::default());
        let context = crate::jobs::TaskContext { group: "g".into(), owner: None, channel: "cli".into(), parent: None, depth: 0 };
        let board = JobBoard::recording(2, log.clone(), context);
        let default = Arc::new(FixedAnswerModel { answer: "d".to_string() });
        let tool = DelegateTool::new(Orchestrator::new(default, temp_vault())).with_models(two_models()).with_jobs(&board).unwrap();

        let result = tool.call(json!({ "task": "x", "model": "strong", "name": "reviewer" })).await.unwrap();

        assert_eq!(result["result"], "answered by strong");
        assert_eq!(*log.0.lock().unwrap(), ["created reviewer model=strong", "finished true"]);
    }

    /// Proves P46's core claim end-to-end: a chain of orchestrators three deep (root → level-1 →
    /// leaf), each wrapping the next via `DelegateTool`, actually lets a sub-agent delegate
    /// further — not just the root. Scripted purely by call order since all three orchestrators
    /// share one model instance (mirrors `warden-bootstrap`'s real wiring: same `model`/`vault`
    /// cloned at every depth) and each `chat_stream` call blocks on any nested delegation before
    /// the next one happens, so the sequence below is deterministic. Also asserts the actual
    /// stopping criterion: the leaf orchestrator's tool specs never include `delegate_task`.
    #[tokio::test]
    async fn supports_bounded_recursive_delegation() {
        struct ScriptedModel {
            call_count: AtomicUsize,
        }

        #[async_trait]
        impl ModelProvider for ScriptedModel {
            async fn chat_stream(&self, _messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                let has_delegate = tools.iter().any(|t| t.name == "delegate_task");
                let response = match self.call_count.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        assert!(has_delegate, "root must be offered delegate_task");
                        Response {
                            content: String::new(),
                            tool_calls: vec![ToolCall {
                                id: "1".to_string(),
                                name: "delegate_task".to_string(),
                                arguments: json!({ "task": "level 2" }),
                                thought_signature: None,
                            }],
                            usage: None,
                        }
                    }
                    1 => {
                        assert!(has_delegate, "level-1 sub-agent must still be offered delegate_task");
                        Response {
                            content: String::new(),
                            tool_calls: vec![ToolCall {
                                id: "2".to_string(),
                                name: "delegate_task".to_string(),
                                arguments: json!({ "task": "level 3" }),
                                thought_signature: None,
                            }],
                            usage: None,
                        }
                    }
                    2 => {
                        assert!(!has_delegate, "leaf orchestrator must NOT be offered delegate_task");
                        Response { content: "leaf answer".to_string(), tool_calls: Vec::new(), usage: None }
                    }
                    3 => Response { content: "level-1 wraps up".to_string(), tool_calls: Vec::new(), usage: None },
                    4 => Response { content: "root wraps up".to_string(), tool_calls: Vec::new(), usage: None },
                    n => panic!("unexpected extra model call {n}"),
                };
                Ok(response_stream(response))
            }
        }

        let model = Arc::new(ScriptedModel { call_count: AtomicUsize::new(0) });
        let vault = temp_vault();

        let leaf = Orchestrator::new(model.clone(), vault.clone());

        let mut level1 = Orchestrator::new(model.clone(), vault.clone());
        level1.register_tool(Arc::new(DelegateTool::new(leaf)));

        let mut root = Orchestrator::new(model.clone(), vault.clone());
        root.register_tool(Arc::new(DelegateTool::new(level1)));

        let outcome = root.handle_message(&[], "level 1").await.unwrap();
        assert_eq!(outcome.content, "root wraps up");
        assert_eq!(model.call_count.load(Ordering::SeqCst), 5);
    }
}
