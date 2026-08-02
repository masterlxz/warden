use std::path::PathBuf;

use serde::Deserialize;
use tauri::State;
use warden_bootstrap::{bootstrap, Overrides};
use warden_core::model::Message;
use warden_core::orchestrator::Orchestrator;

struct AppState {
    orchestrator: Result<Orchestrator, String>,
}

/// Mirrors the frontend's `ChatRole`/`ChatMessage` (`desktop/src/types.ts`) — only the two
/// roles ever shown in the chat UI, since the frontend is what owns conversation history
/// (there's no server-side session to persist it in yet).
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ChatRole {
    User,
    Assistant,
}

#[derive(Deserialize)]
struct ChatTurn {
    role: ChatRole,
    content: String,
}

impl From<ChatTurn> for Message {
    fn from(turn: ChatTurn) -> Self {
        match turn.role {
            ChatRole::User => Message::user(turn.content),
            ChatRole::Assistant => Message::assistant(turn.content),
        }
    }
}

/// A markdown vault is meant to be human-browsable (like an Obsidian vault), unlike opaque
/// app data — so it goes directly under the home dir, not the hidden OS data-dir. A CLI user
/// can rely on their own working directory for the default relative "vault" path; a
/// double-clicked GUI app has no such predictable cwd.
fn desktop_default_vault_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Warden").join("vault")
}

#[tauri::command]
async fn send_message(state: State<'_, AppState>, history: Vec<ChatTurn>, content: String) -> Result<String, String> {
    let orchestrator = state.orchestrator.as_ref().map_err(String::clone)?;
    let history: Vec<Message> = history.into_iter().map(Into::into).collect();
    orchestrator.handle_message(&history, &content).await.map_err(|e| format!("{e:#}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let orchestrator = bootstrap(None, Overrides::default(), desktop_default_vault_path()).map_err(|e| format!("{e:#}"));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { orchestrator })
        .invoke_handler(tauri::generate_handler![send_message])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
