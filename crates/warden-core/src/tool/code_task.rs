//! `code_task` (P89, P124, tension 3 of `VISAO_AGENTES.md`): an agent hands a task to the code engine (the opencode) working
//! in a code project's folder, the way a Programming Manager would. The engine is a tool of the agent, not a second agent
//! of the Warden: the agent says what it wants done, the engine does it in the folder and the agent reads the answer.
//!
//! It is as dangerous as `shell` — the engine edits files and runs commands — so it is held the same way: every ask of
//! the engine goes to the person through `with_approver`'s approver, in `Manual` mode that nobody can loosen from here,
//! and without an approver (a channel that can't ask) the tool refuses.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::code_engine::{CodeEngine, CodeEvent, CodeMode, TurnRequest};
use crate::memory::Vault;
use crate::project::ProjectStore;
use crate::tool::{Approver, Tool, ToolSpec};

/// Where the engine will be, once there is one: the hub builds its tools before the engine (which needs the hub's model
/// route), and builds them again whenever its settings are saved, so the tool holds a slot the hub fills once.
pub type CodeEngineSlot = Arc<OnceLock<Arc<dyn CodeEngine>>>;

pub struct CodeTaskTool {
    projects: ProjectStore,
    engine: CodeEngineSlot,
    approver: Option<Arc<dyn Approver>>,
}

impl CodeTaskTool {
    pub fn new(vault: Arc<Vault>, engine: CodeEngineSlot) -> Self {
        Self { projects: ProjectStore::new(vault), engine, approver: None }
    }

    /// The projects a task can go to: the ones that run on the engine, with their folder.
    fn code_projects(&self) -> Vec<crate::project::Project> {
        self.projects.list().into_iter().filter(|p| p.code && p.workdir.is_some()).collect()
    }
}

#[async_trait]
impl Tool for CodeTaskTool {
    fn spec(&self) -> ToolSpec {
        let projects = self.code_projects();
        let listing = if projects.is_empty() {
            "(none yet: a person has to make a project with a working folder and code mode)".to_string()
        } else {
            projects.iter().map(|p| format!("- {}: {}", p.id, if p.description.is_empty() { &p.name } else { &p.description })).collect::<Vec<_>>().join("\n")
        };
        let mut project = json!({ "type": "string", "description": "Which code project to work in." });
        // An empty `enum` is not valid JSON Schema and some providers refuse the whole request over it.
        if !projects.is_empty() {
            project["enum"] = json!(projects.iter().map(|p| p.id.clone()).collect::<Vec<_>>());
        }
        ToolSpec {
            name: "code_task".to_string(),
            description: format!(
                "Hand a programming task to the code engine (the opencode) working in a code project's folder: it reads and edits \
                 the files and runs commands there, and answers with what it did. The person is asked to approve each of its \
                 actions, so a refused action did not happen. The engine does NOT see this conversation — the 'task' must be \
                 complete and self-contained. To continue the work of an earlier task (same files read, same context), pass the \
                 'session_id' that task returned. Code projects:\n{listing}"
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project": project,
                    "task": {
                        "type": "string",
                        "description": "A complete, self-contained description of the task."
                    },
                    "session_id": {
                        "type": "string",
                        "description": "The session_id a previous code_task returned, to continue that session instead of starting a new one."
                    }
                },
                "required": ["project", "task"]
            }),
        }
    }

    fn with_vault(&self, vault: &Arc<Vault>) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self { projects: ProjectStore::new(vault.clone()), engine: self.engine.clone(), approver: self.approver.clone() }))
    }

    fn with_approver(&self, approver: Arc<dyn Approver>) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self { projects: self.projects.clone(), engine: self.engine.clone(), approver: Some(approver) }))
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let id = args.get("project").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'project' argument"))?;
        let task = args.get("task").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| anyhow::anyhow!("missing required 'task' argument"))?;
        let session_id = args.get("session_id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);

        let projects = self.code_projects();
        let project = projects.iter().find(|p| p.id == id).ok_or_else(|| {
            let available = projects.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ");
            anyhow::anyhow!("'{id}' is not a code project — must be one of: {}", if available.is_empty() { "(none)" } else { &available })
        })?;
        let Some(approver) = self.approver.clone() else {
            anyhow::bail!("the code engine asks the person before each of its actions, and this channel can't ask (use the desktop app or the web)");
        };
        let Some(engine) = self.engine.get().cloned() else {
            anyhow::bail!("the code engine isn't available on this machine");
        };
        let workdir = project.workdir.clone().ok_or_else(|| anyhow::anyhow!("the project '{}' has no working folder", project.name))?;

        // `Manual` for good: nobody holds the sender, so nothing can switch it to "accept all" while the task runs.
        let (_, mode) = tokio::sync::watch::channel(CodeMode::Manual);
        let request = TurnRequest {
            workdir,
            session_id,
            prompt: task.to_string(),
            title: project.name.clone(),
            target: project.name.clone(),
            system: Some(project.instructions.trim().to_string()).filter(|s| !s.is_empty()),
            mode,
        };
        let outcome = engine.run_turn(request, Some(approver), &mut |_: CodeEvent| {}).await?;
        Ok(json!({ "answer": outcome.text, "session_id": outcome.session_id, "tools_used": outcome.tools_used }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::code_engine::{ToolEvent, ToolStatus, TurnOutcome};
    use crate::project::Project;
    use crate::tool::ApprovalRequest;

    fn vault(name: &str) -> Arc<Vault> {
        Arc::new(Vault::new(std::env::temp_dir().join(format!("warden-code-task-{name}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))))
    }

    fn with_projects(name: &str) -> Arc<Vault> {
        let vault = vault(name);
        let store = ProjectStore::new(vault.clone());
        let code = Project { id: "repo".into(), name: "Repo".into(), description: "The main repo".into(), instructions: "Use tabs.".into(), workdir: Some("/home/me/repo".into()), code: true };
        store.save(&code).unwrap();
        store.save(&Project { id: "shell-only".into(), code: false, ..code.clone() }).unwrap();
        store.save(&Project { id: "notes".into(), workdir: None, code: false, ..code }).unwrap();
        vault
    }

    /// An engine that answers from a script, remembers what it was asked and what mode it was told to run in.
    #[derive(Default)]
    struct Scripted {
        asked: Mutex<Vec<(TurnRequest, CodeMode, bool)>>,
    }
    #[async_trait]
    impl CodeEngine for Scripted {
        async fn run_turn(&self, request: TurnRequest, approver: Option<Arc<dyn Approver>>, on_event: &mut (dyn FnMut(CodeEvent) + Send)) -> anyhow::Result<TurnOutcome> {
            on_event(CodeEvent::Tool(ToolEvent { call_id: "c1".into(), tool: "edit".into(), title: "src/lib.rs".into(), status: ToolStatus::Completed }));
            let mode = *request.mode.borrow();
            let session = request.session_id.clone().unwrap_or_else(|| "ses_new".to_string());
            self.asked.lock().unwrap().push((request, mode, approver.is_some()));
            Ok(TurnOutcome { session_id: session, text: "Done.".into(), tools_used: vec!["edit".into()] })
        }
        async fn abort(&self, _: &str, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct Yes;
    #[async_trait]
    impl Approver for Yes {
        async fn approve(&self, _: ApprovalRequest) -> bool {
            true
        }
    }

    fn tool(name: &str, engine: Option<Arc<Scripted>>, asks: bool) -> CodeTaskTool {
        let slot: CodeEngineSlot = Arc::new(OnceLock::new());
        if let Some(engine) = engine {
            let engine: Arc<dyn CodeEngine> = engine;
            slot.set(engine).ok();
        }
        let mut tool = CodeTaskTool::new(with_projects(name), slot);
        if asks {
            tool.approver = Some(Arc::new(Yes));
        }
        tool
    }

    #[test]
    fn only_code_projects_with_a_folder_are_offered() {
        let spec = tool("spec", None, false).spec();
        assert_eq!(spec.parameters["properties"]["project"]["enum"], json!(["repo"]));
        assert!(spec.description.contains("repo: The main repo"));
    }

    #[test]
    fn with_no_code_project_the_schema_has_no_empty_enum() {
        let empty = CodeTaskTool::new(vault("empty"), Arc::new(OnceLock::new()));
        assert!(empty.spec().parameters["properties"]["project"].get("enum").is_none());
    }

    #[tokio::test]
    async fn without_an_approver_it_refuses_and_the_engine_never_runs() {
        let engine = Arc::new(Scripted::default());
        let err = tool("noapprover", Some(engine.clone()), false).call(json!({ "project": "repo", "task": "fix it" })).await.unwrap_err();
        assert!(err.to_string().contains("can't ask"), "{err}");
        assert!(engine.asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_task_runs_in_the_projects_folder_in_manual_mode_with_the_approver() {
        let engine = Arc::new(Scripted::default());
        let result = tool("runs", Some(engine.clone()), true).call(json!({ "project": "repo", "task": "  fix it  " })).await.unwrap();
        assert_eq!(result, json!({ "answer": "Done.", "session_id": "ses_new", "tools_used": ["edit"] }));
        let asked = engine.asked.lock().unwrap();
        let (request, mode, had_approver) = &asked[0];
        assert_eq!((request.workdir.as_str(), request.prompt.as_str(), request.system.as_deref(), request.target.as_str()), ("/home/me/repo", "fix it", Some("Use tabs."), "Repo"));
        assert_eq!(request.session_id, None);
        assert_eq!(*mode, CodeMode::Manual);
        assert!(*had_approver, "the engine's asks must reach the person");
    }

    #[tokio::test]
    async fn a_session_id_continues_the_session() {
        let engine = Arc::new(Scripted::default());
        let result = tool("continues", Some(engine.clone()), true).call(json!({ "project": "repo", "task": "go on", "session_id": "ses_7" })).await.unwrap();
        assert_eq!(result["session_id"], "ses_7");
        assert_eq!(engine.asked.lock().unwrap()[0].0.session_id.as_deref(), Some("ses_7"));
    }

    #[tokio::test]
    async fn a_project_that_is_not_a_code_one_and_a_missing_engine_are_refused() {
        let engine = Arc::new(Scripted::default());
        for project in ["shell-only", "notes", "gone"] {
            let err = tool("notcode", Some(engine.clone()), true).call(json!({ "project": project, "task": "x" })).await.unwrap_err();
            assert!(err.to_string().contains("not a code project"), "{project}: {err}");
        }
        assert!(engine.asked.lock().unwrap().is_empty());
        let err = tool("noengine", None, true).call(json!({ "project": "repo", "task": "x" })).await.unwrap_err();
        assert!(err.to_string().contains("isn't available"), "{err}");
        let err = tool("notask", Some(engine), true).call(json!({ "project": "repo", "task": "   " })).await.unwrap_err();
        assert!(err.to_string().contains("'task'"), "{err}");
    }
}
