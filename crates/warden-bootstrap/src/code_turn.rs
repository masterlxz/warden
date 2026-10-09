//! The turn of a code project's conversation (P103 b): instead of the Warden's own turn (`handle_agent_turn`), the
//! message is a task for a code engine (the opencode) working in the project's folder. The conversation stays an
//! ordinary Warden conversation — the same file, the same list, the same project — and keeps the engine's session id
//! so the next message goes to the same session; what the engine did (files, commands) lives in the folder and in the
//! session, and the conversation keeps what was said and which tools were used.

use std::path::Path;
use std::sync::Arc;

use warden_core::code_engine::{CodeEngine, CodeEvent, CodeMode, TurnRequest};
use warden_core::model::Attachment;
use warden_core::orchestrator::MessageOutcome;
use warden_core::project::{Project, ProjectStore};
use warden_core::tool::Approver;

use super::{append_messages, load_conversation, message_id, now_millis, AppendOptions, ChatRole, ConversationMessage};

/// The code project a turn of this conversation would run in, if it is one: the project the conversation was created in
/// (a conversation that exists keeps its own, whatever a client sends later), else the one being asked for. A project
/// that doesn't exist, has no working folder or isn't a code project gives `None`, and the turn is the ordinary one.
pub fn code_project(projects: &ProjectStore, conversations_dir: &Path, conversation_id: &str, requested: Option<&str>) -> anyhow::Result<Option<Project>> {
    let id = match load_conversation(conversations_dir, conversation_id)? {
        Some(conversation) => conversation.project_id,
        None => requested.map(str::to_string),
    };
    Ok(id.and_then(|id| projects.get(&id).ok()).filter(|project| project.code && project.workdir.is_some()))
}

pub struct CodeTurn<'a> {
    pub engine: &'a dyn CodeEngine,
    pub project: &'a Project,
    pub conversations_dir: &'a Path,
    pub conversation_id: &'a str,
    pub title_seed: &'a str,
    pub user_input: &'a str,
    pub attachments: Vec<Attachment>,
    /// How much to ask the person, changeable while the task runs (`CodeModes::subscribe`).
    pub mode: tokio::sync::watch::Receiver<CodeMode>,
}

impl CodeTurn<'_> {
    /// Runs the task, telling `on_event` what the engine does as it does it, and saves the exchange. A task that
    /// fails saves nothing, like the ordinary turn: the failure is the caller's to show.
    pub async fn run(self, approver: Option<Arc<dyn Approver>>, on_event: &mut (dyn FnMut(CodeEvent) + Send)) -> anyhow::Result<MessageOutcome> {
        anyhow::ensure!(self.attachments.is_empty(), "a code project's conversation doesn't take attachments yet");
        let workdir = self.project.workdir.clone().ok_or_else(|| anyhow::anyhow!("the project '{}' has no working folder", self.project.name))?;
        let existing = load_conversation(self.conversations_dir, self.conversation_id)?;
        let request = TurnRequest {
            workdir,
            session_id: existing.as_ref().and_then(|c| c.engine_session_id.clone()),
            prompt: self.user_input.to_string(),
            title: self.project.name.clone(),
            target: self.project.name.clone(),
            system: Some(self.project.instructions.trim().to_string()).filter(|s| !s.is_empty()),
            mode: self.mode,
        };
        let outcome = self.engine.run_turn(request, approver, on_event).await?;

        let message = |role, content: String, tools_used: Vec<String>| ConversationMessage {
            id: message_id(),
            role,
            content,
            created_at: now_millis(),
            usage: None,
            answered_by: None,
            attachments: Vec::new(),
            generated_files: Vec::new(),
            tools_used,
        };
        let messages = vec![message(ChatRole::User, self.user_input.to_string(), Vec::new()), message(ChatRole::Assistant, outcome.text.clone(), outcome.tools_used.clone())];
        let options = AppendOptions {
            title_seed: self.title_seed,
            project_id: existing.is_none().then_some(self.project.id.as_str()),
            engine_session_id: Some(&outcome.session_id),
            create: existing.is_none(),
            ..Default::default()
        };
        append_messages(self.conversations_dir, self.conversation_id, options, messages)?;
        Ok(MessageOutcome {
            content: outcome.text,
            usage: None,
            attachments: Vec::new(),
            generated_files: Vec::new(),
            fallbacks: Vec::new(),
            client_tool_calls: Vec::new(),
            tools_used: outcome.tools_used,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use warden_core::code_engine::{ToolEvent, ToolStatus, TurnOutcome};
    use warden_core::memory::Vault;

    use super::*;
    use crate::set_conversation_project;

    fn dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("warden-code-turn-{name}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
    }

    fn projects(name: &str) -> ProjectStore {
        let store = ProjectStore::new(Arc::new(Vault::new(dir(name).join("vault"))));
        let code = Project { id: "repo".into(), name: "Repo".into(), description: String::new(), instructions: "Use tabs.".into(), workdir: Some("/home/me/repo".into()), code: true };
        store.save(&code).unwrap();
        store.save(&Project { id: "shell-only".into(), workdir: Some("/home/me/other".into()), code: false, ..code.clone() }).unwrap();
        store.save(&Project { id: "notes".into(), workdir: None, code: false, ..code }).unwrap();
        store
    }

    /// An engine that answers from a script and remembers what it was asked.
    struct Scripted {
        asked: Mutex<Vec<TurnRequest>>,
        fails: bool,
    }
    #[async_trait]
    impl CodeEngine for Scripted {
        async fn run_turn(&self, request: TurnRequest, _: Option<Arc<dyn Approver>>, on_event: &mut (dyn FnMut(CodeEvent) + Send)) -> anyhow::Result<TurnOutcome> {
            self.asked.lock().unwrap().push(request);
            anyhow::ensure!(!self.fails, "the engine failed");
            on_event(CodeEvent::Tool(ToolEvent { call_id: "c1".into(), tool: "edit".into(), title: "src/lib.rs".into(), status: ToolStatus::Completed }));
            on_event(CodeEvent::Text("Done.".into()));
            Ok(TurnOutcome { session_id: "ses_1".into(), text: "Done.".into(), tools_used: vec!["edit".into()] })
        }
        async fn abort(&self, _: &str, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }
    fn engine(fails: bool) -> Scripted {
        Scripted { asked: Mutex::default(), fails }
    }

    fn turn<'a>(engine: &'a Scripted, project: &'a Project, dir: &'a Path, text: &'a str) -> CodeTurn<'a> {
        CodeTurn { engine, project, conversations_dir: dir, conversation_id: "c1", title_seed: text, user_input: text, attachments: Vec::new(), mode: tokio::sync::watch::channel(CodeMode::Manual).1 }
    }

    #[test]
    fn only_a_code_project_with_a_folder_sends_its_conversations_to_the_engine() {
        let (store, conversations) = (projects("which"), dir("which-conversations"));
        let pick = |requested: Option<&str>| code_project(&store, &conversations, "c1", requested).unwrap().map(|p| p.id);
        assert_eq!(pick(Some("repo")).as_deref(), Some("repo"));
        assert_eq!(pick(Some("shell-only")), None, "a working folder alone keeps the ordinary turn with its shell");
        assert_eq!(pick(Some("notes")), None);
        assert_eq!(pick(Some("gone")), None);
        assert_eq!(pick(None), None);
    }

    #[tokio::test]
    async fn a_conversation_that_exists_keeps_its_own_project_whatever_is_asked_later() {
        let (store, conversations) = (projects("keeps"), dir("keeps-conversations"));
        let project = store.get("repo").unwrap();
        turn(&engine(false), &project, &conversations, "first").run(None, &mut |_| {}).await.unwrap();
        assert_eq!(code_project(&store, &conversations, "c1", None).unwrap().map(|p| p.id).as_deref(), Some("repo"), "no project sent, still its own");
        assert_eq!(code_project(&store, &conversations, "c1", Some("notes")).unwrap().map(|p| p.id).as_deref(), Some("repo"));
    }

    #[tokio::test]
    async fn the_first_task_opens_the_session_and_the_next_one_goes_to_the_same_one() {
        let (store, conversations) = (projects("session"), dir("session-conversations"));
        let (project, engine) = (store.get("repo").unwrap(), engine(false));
        let mut seen = Vec::new();
        let first = turn(&engine, &project, &conversations, "add a test").run(None, &mut |e| seen.push(e)).await.unwrap();
        assert_eq!(first.content, "Done.");
        assert_eq!(first.tools_used, ["edit"]);
        assert_eq!(seen.len(), 2, "what the engine did is shown as it happens");

        let saved = load_conversation(&conversations, "c1").unwrap().unwrap();
        assert_eq!((saved.project_id.as_deref(), saved.engine_session_id.as_deref()), (Some("repo"), Some("ses_1")));
        assert_eq!(saved.messages.len(), 2);
        assert_eq!((saved.messages[0].role, saved.messages[0].content.as_str()), (ChatRole::User, "add a test"));
        assert_eq!(saved.messages[1].role, ChatRole::Assistant);
        assert_eq!(saved.messages[1].tools_used, ["edit"]);

        turn(&engine, &project, &conversations, "now run it").run(None, &mut |_| {}).await.unwrap();
        let asked = engine.asked.lock().unwrap();
        assert_eq!(asked[0].session_id, None);
        assert_eq!(asked[1].session_id.as_deref(), Some("ses_1"));
        assert_eq!((asked[0].workdir.as_str(), asked[0].system.as_deref(), asked[0].target.as_str()), ("/home/me/repo", Some("Use tabs."), "Repo"));
        assert_eq!(load_conversation(&conversations, "c1").unwrap().unwrap().messages.len(), 4);
    }

    #[tokio::test]
    async fn a_failed_task_and_an_attachment_save_nothing() {
        let (store, conversations) = (projects("fails"), dir("fails-conversations"));
        let project = store.get("repo").unwrap();
        let err = turn(&engine(true), &project, &conversations, "go").run(None, &mut |_| {}).await.err().unwrap();
        assert!(err.to_string().contains("the engine failed"));
        let working = engine(false);
        let with_file = CodeTurn { attachments: vec![Attachment { mime_type: "image/png".into(), data: "AA==".into() }], ..turn(&working, &project, &conversations, "look") };
        assert!(with_file.run(None, &mut |_| {}).await.err().unwrap().to_string().contains("attachments"));
        assert!(load_conversation(&conversations, "c1").unwrap().is_none());
    }

    #[tokio::test]
    async fn moving_the_conversation_to_another_project_forgets_the_engines_session() {
        let (store, conversations) = (projects("move"), dir("move-conversations"));
        turn(&engine(false), &store.get("repo").unwrap(), &conversations, "go").run(None, &mut |_| {}).await.unwrap();
        set_conversation_project(&conversations, "c1", Some("repo")).unwrap();
        assert_eq!(load_conversation(&conversations, "c1").unwrap().unwrap().engine_session_id.as_deref(), Some("ses_1"), "staying put keeps it");
        set_conversation_project(&conversations, "c1", Some("notes")).unwrap();
        assert_eq!(load_conversation(&conversations, "c1").unwrap().unwrap().engine_session_id, None);
    }
}
