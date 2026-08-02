use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::State;
use warden_bootstrap::{
    bootstrap, default_config_path, default_model_for, load_config_from_path, save_config, ApiKeys, FileConfig,
    Overrides, Provider,
};
use warden_core::model::Message;
use warden_core::orchestrator::Orchestrator;

struct AppState {
    orchestrator: Mutex<Result<Orchestrator, String>>,
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
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    let history: Vec<Message> = history.into_iter().map(Into::into).collect();
    orchestrator.handle_message(&history, &content).await.map_err(|e| format!("{e:#}"))
}

/// What the settings screen reads. API keys are returned in plain text (the user's own explicit
/// choice for this app — shown masked with a reveal toggle client-side) rather than just a
/// "configured" boolean, since the form is meant to be directly editable. Empty string means
/// "not set" throughout, mirroring the `ChatTurn` convention of keeping the IPC boundary in
/// plain strings instead of `Option`/`null`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsSnapshot {
    provider: Provider,
    model: String,
    vault_path: String,
    default_model_gemini: String,
    default_model_openai: String,
    gemini_key: String,
    openai_key: String,
    tavily_key: String,
}

#[derive(Deserialize)]
struct SettingsFormPayload {
    provider: String,
    model: String,
    vault_path: String,
    gemini_key: String,
    openai_key: String,
    tavily_key: String,
}

#[tauri::command]
fn get_settings() -> Result<SettingsSnapshot, String> {
    let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;

    Ok(SettingsSnapshot {
        provider: config.provider.unwrap_or(Provider::Gemini),
        model: config.model.unwrap_or_default(),
        vault_path: config.vault_path.unwrap_or_default(),
        default_model_gemini: default_model_for(Provider::Gemini).to_string(),
        default_model_openai: default_model_for(Provider::Openai).to_string(),
        gemini_key: config.api_keys.gemini.unwrap_or_default(),
        openai_key: config.api_keys.openai.unwrap_or_default(),
        tavily_key: config.api_keys.tavily.unwrap_or_default(),
    })
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, payload: SettingsFormPayload) -> Result<(), String> {
    fn non_empty(s: String) -> Option<String> {
        let trimmed = s.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    let provider = match payload.provider.as_str() {
        "gemini" => Provider::Gemini,
        "openai" => Provider::Openai,
        other => return Err(format!("unknown provider: {other}")),
    };

    let config = FileConfig {
        provider: Some(provider),
        model: non_empty(payload.model),
        vault_path: non_empty(payload.vault_path),
        api_keys: ApiKeys {
            gemini: non_empty(payload.gemini_key),
            openai: non_empty(payload.openai_key),
            tavily: non_empty(payload.tavily_key),
        },
    };

    let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;

    let new_orchestrator = bootstrap(None, Overrides::default(), desktop_default_vault_path()).map_err(|e| format!("{e:#}"));
    *state.orchestrator.lock().unwrap() = new_orchestrator;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let orchestrator = bootstrap(None, Overrides::default(), desktop_default_vault_path()).map_err(|e| format!("{e:#}"));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { orchestrator: Mutex::new(orchestrator) })
        .invoke_handler(tauri::generate_handler![send_message, get_settings, save_settings])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
