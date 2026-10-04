//! The desktop's own chat running a code project's conversation (P103 b): the same task for the opencode that the hub
//! runs for the web (`warden_bootstrap::code_turn::CodeTurn`), but in this process, so it doesn't need the embedded hub
//! turned on. What the engine does reaches the window as `chat-event`s, its asks go to the same modal as every other
//! approval, and `cancel_turn` stops it.

use std::sync::Arc;

use serde_json::json;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::OnceCell;
use warden_bootstrap::code_turn::CodeTurn;
use warden_core::code_engine::{CodeEngine, CodeEvent, CodeMode, CodeModes};
use warden_core::model::Attachment;
use warden_core::orchestrator::Orchestrator;
use warden_core::project::Project;
use warden_server::code_turns::{opencode_engine, CodeTurns};
use warden_server::engine_models::EngineModels;
use warden_server::SharedOrchestrator;
use warden_server_protocol::protocol::ChatEventDto;

use crate::{approval, AppState};

/// What a turn of code needs, started with the first one: a hub with no code project never runs the opencode.
struct Runtime {
    /// The model the opencode talks to goes through this one, which follows the desktop's settings (see `run_turn`).
    shared: SharedOrchestrator,
    engine: Arc<dyn CodeEngine>,
}

#[derive(Default)]
pub struct CodeState {
    runtime: OnceCell<Runtime>,
    /// The tasks running now, to find the session `cancel_turn` stops.
    turns: CodeTurns,
    /// How much each conversation asks, changeable while its task runs (`set_code_mode`).
    modes: CodeModes,
}

impl CodeState {
    /// `mode` is a name a window sent (`CodeMode::parse`).
    pub fn set_mode(&self, conversation_id: &str, mode: &str) {
        self.modes.set(conversation_id, CodeMode::parse(mode));
    }
}

/// Runs `content` as a task in `project`'s folder and returns what the engine answered. The exchange is already saved
/// in the conversation when this returns.
pub async fn run_turn(
    app: AppHandle,
    state: &AppState,
    base: &Orchestrator,
    project: Project,
    conversation_id: &str,
    content: &str,
    attachments: Vec<Attachment>,
) -> Result<String, String> {
    let runtime = state
        .code
        .runtime
        .get_or_try_init(|| async {
            let shared = SharedOrchestrator::new(base.clone());
            let models = EngineModels::start(shared.clone()).await?;
            Ok::<_, anyhow::Error>(Runtime { shared, engine: opencode_engine(&models) })
        })
        .await
        .map_err(|e| format!("{e:#}"))?;
    // A provider changed in Settings since the last task reaches the opencode's next request.
    runtime.shared.replace(base.clone());

    let conversations_dir = warden_bootstrap::default_conversations_dir().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let turns = state.code.turns.clone();
    turns.begin(conversation_id, project.workdir.as_deref().unwrap_or_default());
    let (event_app, event_turns, event_id) = (app.clone(), turns.clone(), conversation_id.to_string());
    let mut on_event = move |event: CodeEvent| match event {
        CodeEvent::Session(session) => event_turns.set_session(&event_id, session),
        other => {
            if let Some(event) = ChatEventDto::from_code(other) {
                let _ = event_app.emit("chat-event", json!({ "conversationId": event_id, "event": event }));
            }
        }
    };
    let seed = warden_server::chat_input::title_seed(content, &attachments);
    let turn = CodeTurn {
        engine: runtime.engine.as_ref(),
        project: &project,
        conversations_dir: &conversations_dir,
        conversation_id,
        title_seed: &seed,
        user_input: content,
        attachments,
        mode: state.code.modes.subscribe(conversation_id),
    };
    let approver = Arc::new(approval::TauriApprover { app, broker: state.approvals.clone() });
    let result = turn.run(Some(approver), &mut on_event).await;
    turns.end(conversation_id);
    result.map(|outcome| outcome.content).map_err(|e| format!("{e:#}"))
}

/// How much this conversation asks before the engine acts: `manual`, `acceptEdits`, `acceptAll` or `plan`. Takes effect
/// at once, a task that is running included.
#[tauri::command]
pub fn set_code_mode(state: State<'_, AppState>, conversation_id: String, mode: String) {
    state.code.set_mode(&conversation_id, &mode);
}

/// The Stop button: asks the engine to stop the task this conversation is running. Harmless when it isn't running one.
#[tauri::command]
pub async fn cancel_turn(state: State<'_, AppState>, conversation_id: String) -> Result<(), String> {
    let (Some((workdir, session)), Some(runtime)) = (state.code.turns.session_of(&conversation_id), state.code.runtime.get()) else {
        return Ok(());
    };
    runtime.engine.abort(&workdir, &session).await.map_err(|e| format!("{e:#}"))
}
