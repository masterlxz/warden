mod recording;
mod sync_cmds;
mod vault_cmds;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::State;
use warden_bootstrap::{
    aggregate_usage, bootstrap, build_delegate_to_agent_tool, build_model_provider, default_config_path, default_conversations_dir,
    default_model_for, list_conversations as read_conversations, load_config, load_config_from_path, oauth_credential_store_path,
    resolve_vault_path, save_config, save_conversation as write_conversation, AgentConfig, ApiKeys, Conversation, FileConfig,
    McpServerConfig, Overrides, Provider, ProviderConfig, UsageSummary,
};
use warden_core::model::{Attachment, Message};
use warden_core::orchestrator::Orchestrator;

struct AppState {
    orchestrator: Mutex<Result<Orchestrator, String>>,
    /// Set between a `start_recording`/`stop_recording` pair (P28) — `None` otherwise.
    recording: Mutex<Option<recording::ActiveRecording>>,
    /// P37 — the vault+config sync engine (Arweave via TruthID). No `Mutex` around the engine
    /// itself: every method takes `&self` and does its own file I/O, nothing mutates in memory.
    sync: warden_sync::SyncEngine,
    /// Set by `sync_cmds::sync_push_begin`, taken by `sync_cmds::sync_push_await` — showing the
    /// QR and blocking on the TruthID phone are deliberately separate IPC calls.
    pending_push: Mutex<Option<warden_sync::push::BeginPushResult>>,
}

/// Mirrors the frontend's `ChatRole`/`ChatMessage` (`desktop/src/types.ts`) — only the two
/// roles ever shown in the chat UI. Deliberately separate from `warden_bootstrap::Conversation`
/// (which is what actually gets persisted, see `list_conversations`/`save_conversation` below):
/// `send_message`'s `history` param only ever needs role+content, not the id/timestamps a
/// persisted message carries.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ChatRole {
    User,
    Assistant,
}

/// Mirrors the frontend's `Attachment` (`desktop/src/types.ts`) — an inline image (P28),
/// base64-encoded with no `data:...;base64,` prefix.
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AttachmentPayload {
    mime_type: String,
    data: String,
}

impl From<AttachmentPayload> for Attachment {
    fn from(a: AttachmentPayload) -> Self {
        Attachment { mime_type: a.mime_type, data: a.data }
    }
}

#[derive(Deserialize)]
struct ChatTurn {
    role: ChatRole,
    content: String,
    #[serde(default)]
    attachments: Vec<AttachmentPayload>,
}

impl From<ChatTurn> for Message {
    fn from(turn: ChatTurn) -> Self {
        match turn.role {
            ChatRole::User => Message::user_with_attachments(turn.content, turn.attachments.into_iter().map(Into::into).collect()),
            ChatRole::Assistant => Message::assistant(turn.content),
        }
    }
}

/// Extensions this build accepts for an attached image — kept in sync with the file dialog
/// filter the frontend opens (`MessageInput.tsx`). Anything else is rejected here rather than
/// silently forwarded to a model provider that wouldn't know what to do with it.
fn image_mime_type_for_extension(extension: &str) -> Option<&'static str> {
    match extension.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// Reads a local image file picked from the attach button's file dialog and returns it
/// base64-encoded, ready to hand back to `send_message` as an `AttachmentPayload` (P28).
#[tauri::command]
fn read_attachment(path: String) -> Result<AttachmentPayload, String> {
    let path = PathBuf::from(path);
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let mime_type = image_mime_type_for_extension(extension).ok_or_else(|| "Unsupported file type".to_string())?;

    let bytes = std::fs::read(&path).map_err(|e| format!("{e:#}"))?;
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);

    Ok(AttachmentPayload { mime_type: mime_type.to_string(), data })
}

/// A markdown vault is meant to be human-browsable (like an Obsidian vault), unlike opaque
/// app data — so it goes directly under the home dir, not the hidden OS data-dir. A CLI user
/// can rely on their own working directory for the default relative "vault" path; a
/// double-clicked GUI app has no such predictable cwd.
fn desktop_default_vault_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Warden").join("vault")
}

/// What `send_message` hands back over IPC — the frontend's `ChatMessage.usage` (`desktop/src/
/// types.ts`) mirrors this field for field.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageResult {
    content: String,
    usage: Option<warden_core::model::Usage>,
}

/// `agent_id`/`provider_id` are the per-conversation selectors (closes P3) — the frontend sends
/// an explicit `null` for either when not overriding, rather than omitting the key, so this stays
/// unambiguous `Option<String>` deserialization. Both only ever affect this one call: `history`
/// (built fresh from the conversation's stored messages each time) is what makes a mid-conversation
/// switch apply "from here on" without needing to touch anything already said.
#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    history: Vec<ChatTurn>,
    content: String,
    attachments: Vec<AttachmentPayload>,
    agent_id: Option<String>,
    provider_id: Option<String>,
) -> Result<SendMessageResult, String> {
    let mut orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    let history: Vec<Message> = history.into_iter().map(Into::into).collect();
    let attachments: Vec<Attachment> = attachments.into_iter().map(Into::into).collect();

    let mut persona = None;
    if agent_id.is_some() || provider_id.is_some() {
        let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
        let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;

        if let Some(id) = &agent_id {
            if let Some(agent) = config.agents.iter().find(|a| &a.id == id) {
                persona = Some(agent.persona.clone());
                if agent.can_delegate_to_agents {
                    if let Some(tool) = build_delegate_to_agent_tool(&config, &orchestrator) {
                        orchestrator = orchestrator.with_tool(tool);
                    }
                }
            }
        }
        if let Some(id) = &provider_id {
            let provider = config.providers.iter().find(|p| &p.id == id).ok_or_else(|| format!("model provider '{id}' not found"))?;
            let model = build_model_provider(provider, None).map_err(|e| format!("{e:#}"))?;
            orchestrator = orchestrator.with_model(model);
        }
    }

    let outcome =
        orchestrator.handle_turn(&history, &content, attachments, persona.as_deref()).await.map_err(|e| format!("{e:#}"))?;
    Ok(SendMessageResult { content: outcome.content, usage: outcome.usage })
}

/// Filename handed to the Whisper API for a recorded clip — only the extension matters (the API
/// infers format from it). `stop_recording` always produces `audio/wav` (native capture, P28);
/// the other cases are kept for robustness in case that ever changes.
fn audio_filename_for_mime_type(mime_type: &str) -> &'static str {
    match mime_type {
        "audio/webm" => "audio.webm",
        "audio/ogg" => "audio.ogg",
        "audio/mp4" => "audio.mp4",
        "audio/wav" => "audio.wav",
        "audio/mpeg" => "audio.mp3",
        _ => "audio.webm",
    }
}

/// Transcribes a voice recording from the composer's mic button (P28 part 2) — always via a
/// dedicated Whisper API key, independent of which chat provider is active, so voice input works
/// the same regardless of whether Gemini/OpenAI/Anthropic is selected.
#[tauri::command]
async fn transcribe_audio(audio: AttachmentPayload) -> Result<String, String> {
    let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    let api_key = config
        .api_keys
        .whisper
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "Set a Whisper API key in Settings to enable voice input".to_string())?;

    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &audio.data).map_err(|e| format!("{e:#}"))?;
    let filename = audio_filename_for_mime_type(&audio.mime_type);

    warden_core::transcribe::transcribe_audio(&api_key, bytes, filename).await.map_err(|e| format!("{e:#}"))
}

/// Synthesizes speech from an assistant message's text (P28 part 3, the per-message speaker
/// button) — reuses the same Whisper API key as `transcribe_audio`, since both are OpenAI audio
/// endpoints on the same account.
#[tauri::command]
async fn synthesize_speech(text: String) -> Result<AttachmentPayload, String> {
    let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    let api_key = config
        .api_keys
        .whisper
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "Set a Whisper API key in Settings to enable text-to-speech".to_string())?;

    let bytes = warden_core::speech::synthesize_speech(&api_key, &text).await.map_err(|e| format!("{e:#}"))?;
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    Ok(AttachmentPayload { mime_type: "audio/mpeg".to_string(), data })
}

/// Starts native mic capture for the composer's record button (P28) — see `recording` module's
/// doc comment for why this doesn't use the browser's `getUserMedia` instead. Fails immediately
/// if there's no microphone, rather than only once `stop_recording` is called.
#[tauri::command]
fn start_recording(state: State<'_, AppState>) -> Result<(), String> {
    let mut slot = state.recording.lock().unwrap();
    if slot.is_some() {
        return Err("Already recording".to_string());
    }
    *slot = Some(recording::start()?);
    Ok(())
}

/// Stops the active recording and returns it as a WAV `AttachmentPayload`, ready to hand to
/// `transcribe_audio`.
#[tauri::command]
fn stop_recording(state: State<'_, AppState>) -> Result<AttachmentPayload, String> {
    let active = state.recording.lock().unwrap().take().ok_or_else(|| "Not recording".to_string())?;
    let (samples, sample_rate) = active.stop()?;
    let wav_bytes = recording::encode_wav(&samples, sample_rate)?;
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, wav_bytes);
    Ok(AttachmentPayload { mime_type: "audio/wav".to_string(), data })
}

/// One entry of the provider registry (Sessão 35), as read/written by the Settings screen.
/// API keys are returned in plain text (the user's own explicit choice for this app — shown
/// masked with a reveal toggle client-side) rather than just a "configured" boolean, since the
/// form is meant to be directly editable. Empty string means "not set" throughout, mirroring the
/// `ChatTurn` convention of keeping the IPC boundary in plain strings instead of `Option`/`null`.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProviderPayload {
    id: String,
    kind: Provider,
    api_key: String,
    base_url: String,
    model: String,
}

/// One entry of the agent registry (closes P3), as read/written by the Settings screen — same
/// "flat list, `id` doubles as display name" shape as `ProviderPayload`. `provider_id` empty
/// string means "no default model" (mirrors every other "not set" field in this file).
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentPayload {
    id: String,
    persona: String,
    provider_id: String,
    /// Opt-in (P46/P60) for the `delegate_to_agent` tool — see `AgentConfig::can_delegate_to_agents`.
    can_delegate_to_agents: bool,
}

/// What the settings screen reads.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsSnapshot {
    providers: Vec<ProviderPayload>,
    active_provider: String,
    vault_path: String,
    tavily_key: String,
    /// OpenAI API key for Whisper transcription (P28 part 2) — dedicated, independent of which
    /// provider is active for chat. Same "not set" = empty string convention as `tavily_key`.
    whisper_key: String,
    enable_shell: bool,
    /// Default model per provider kind, keyed by the same string the frontend uses for `kind`
    /// (`"gemini"`/`"openai"`/`"anthropic"`) — shown as the Model field's placeholder. No entry
    /// for `openai_compatible`, which has no universal default (see `default_model_for`).
    default_models: std::collections::HashMap<String, String>,
    /// External MCP servers (Phase 5.2/P25) — `McpServerConfig`'s own fields, for either
    /// transport (`name`/`command`/`args`/`env` for stdio, `name`/`url`/`headers` for HTTP), are
    /// already single-word, so the untagged enum round-trips over IPC as-is with no dedicated
    /// payload type (unlike `ProviderPayload`, which needed one for the `camelCase` API key
    /// field names).
    mcp_servers: Vec<McpServerConfig>,
    /// The agent registry (closes P3) — named personas a conversation can pick, alongside its
    /// model.
    agents: Vec<AgentPayload>,
}

#[derive(Deserialize)]
struct SettingsFormPayload {
    providers: Vec<ProviderPayload>,
    active_provider: String,
    vault_path: String,
    tavily_key: String,
    whisper_key: String,
    enable_shell: bool,
    mcp_servers: Vec<McpServerConfig>,
    agents: Vec<AgentPayload>,
}

fn default_models_by_kind() -> std::collections::HashMap<String, String> {
    [("gemini", Provider::Gemini), ("openai", Provider::Openai), ("anthropic", Provider::Anthropic)]
        .into_iter()
        .filter_map(|(key, kind)| default_model_for(kind).map(|model| (key.to_string(), model.to_string())))
        .collect()
}

#[tauri::command]
fn get_settings() -> Result<SettingsSnapshot, String> {
    let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;

    Ok(SettingsSnapshot {
        providers: config
            .providers
            .into_iter()
            .map(|p| ProviderPayload {
                id: p.id,
                kind: p.kind,
                api_key: p.api_key.unwrap_or_default(),
                base_url: p.base_url.unwrap_or_default(),
                model: p.model.unwrap_or_default(),
            })
            .collect(),
        active_provider: config.active_provider.unwrap_or_default(),
        vault_path: config.vault_path.unwrap_or_default(),
        tavily_key: config.api_keys.tavily.unwrap_or_default(),
        whisper_key: config.api_keys.whisper.unwrap_or_default(),
        enable_shell: config.enable_shell.unwrap_or(false),
        default_models: default_models_by_kind(),
        mcp_servers: config.mcp_servers,
        agents: config
            .agents
            .into_iter()
            .map(|a| AgentPayload {
                id: a.id,
                persona: a.persona,
                provider_id: a.provider_id.unwrap_or_default(),
                can_delegate_to_agents: a.can_delegate_to_agents,
            })
            .collect(),
    })
}

#[tauri::command]
async fn save_settings(state: State<'_, AppState>, payload: SettingsFormPayload) -> Result<(), String> {
    fn non_empty(s: String) -> Option<String> {
        let trimmed = s.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    // The Telegram bot token (Fase 2) and `delegate_max_depth` (P46/P60) have no Settings-screen
    // UI yet (see PENDING.md P11) — only hand-editable via config.toml. Loaded up front so every
    // "carry forward instead of wiping" field below can reference it.
    let existing = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;

    let mut providers = Vec::with_capacity(payload.providers.len());
    let mut seen_ids = std::collections::HashSet::new();
    for p in payload.providers {
        let id = p.id.trim().to_string();
        if id.is_empty() {
            return Err("every provider needs a name".to_string());
        }
        if !seen_ids.insert(id.clone()) {
            return Err(format!("duplicate provider name: {id}"));
        }
        providers.push(ProviderConfig { id, kind: p.kind, api_key: non_empty(p.api_key), base_url: non_empty(p.base_url), model: non_empty(p.model) });
    }

    let mut mcp_servers = Vec::with_capacity(payload.mcp_servers.len());
    for s in payload.mcp_servers {
        mcp_servers.push(match s {
            McpServerConfig::Stdio { name, command, args, env } => {
                let name = name.trim().to_string();
                let command = command.trim().to_string();
                if name.is_empty() {
                    return Err("every MCP server needs a name".to_string());
                }
                if command.is_empty() {
                    return Err(format!("MCP server '{name}' needs a command"));
                }
                McpServerConfig::Stdio { name, command, args, env }
            }
            McpServerConfig::Http { name, url, headers, oauth } => {
                let name = name.trim().to_string();
                let url = url.trim().to_string();
                if name.is_empty() {
                    return Err("every MCP server needs a name".to_string());
                }
                if url.is_empty() {
                    return Err(format!("MCP server '{name}' needs a URL"));
                }
                McpServerConfig::Http { name, url, headers, oauth }
            }
        });
    }

    let mut agents = Vec::with_capacity(payload.agents.len());
    let mut seen_agent_ids = std::collections::HashSet::new();
    for a in payload.agents {
        let id = a.id.trim().to_string();
        if id.is_empty() {
            return Err("every agent needs a name".to_string());
        }
        if !seen_agent_ids.insert(id.clone()) {
            return Err(format!("duplicate agent name: {id}"));
        }
        let provider_id = non_empty(a.provider_id);
        if let Some(pid) = &provider_id {
            if !providers.iter().any(|p| &p.id == pid) {
                return Err(format!("agent '{id}' has an unknown default provider '{pid}'"));
            }
        }
        agents.push(AgentConfig { id, persona: a.persona, provider_id, can_delegate_to_agents: a.can_delegate_to_agents });
    }

    let active_provider = non_empty(payload.active_provider);
    if let Some(active_id) = &active_provider {
        if !providers.iter().any(|p| &p.id == active_id) {
            return Err(format!("active provider '{active_id}' is not one of the configured providers"));
        }
    }

    let config = FileConfig {
        // The legacy single-provider fields are only ever read as a fallback when `providers`
        // is empty (see `resolve_model_provider` in warden-bootstrap) — once this screen has
        // saved at least once, the registry below is authoritative, so clear them instead of
        // leaving stale duplicate secrets sitting in the file.
        provider: None,
        model: None,
        vault_path: non_empty(payload.vault_path),
        enable_shell: Some(payload.enable_shell),
        // No Settings-screen UI yet (P46, config.toml/env-only advanced knob) — carry forward
        // whatever was on disk instead of wiping it, same reasoning as `telegram_bot_token` above.
        delegate_max_depth: existing.delegate_max_depth,
        api_keys: ApiKeys {
            gemini: None,
            openai: None,
            tavily: non_empty(payload.tavily_key),
            telegram_bot_token: existing.api_keys.telegram_bot_token,
            whisper: non_empty(payload.whisper_key),
        },
        providers,
        active_provider,
        mcp_servers,
        agents,
        // No Settings-screen UI yet (P61, config.toml/env-only advanced knob) — carry forward
        // whatever was on disk instead of wiping it, same reasoning as `delegate_max_depth` above.
        storage_provider: existing.storage_provider,
    };

    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;

    let new_orchestrator = bootstrap(None, Overrides::default(), desktop_default_vault_path()).await.map_err(|e| format!("{e:#}"));
    *state.orchestrator.lock().unwrap() = new_orchestrator;
    Ok(())
}

/// Whether an OAuth-authenticated MCP server (PENDING.md P26) already has a token on disk.
/// No network call — matches the "no dedicated health-check system" scope every other MCP
/// server transport has today; a truly expired/unrefreshable token is only surfaced the next
/// time `bootstrap()` actually tries to connect.
#[tauri::command]
fn mcp_oauth_status(name: String) -> bool {
    oauth_credential_store_path(&name).is_file()
}

/// Runs the interactive OAuth authorization flow for one HTTP MCP server (Settings' "Connect"
/// button) — opens the system browser via `tauri_plugin_opener`, waits for the redirect, and
/// persists the resulting token. Safe to call again on an already-connected server: it
/// short-circuits without touching the browser (see `authorize_interactively`'s doc comment).
#[tauri::command]
async fn mcp_oauth_connect(state: State<'_, AppState>, name: String, url: String) -> Result<(), String> {
    let credential_store_path = oauth_credential_store_path(&name);
    warden_core::tool::mcp_oauth::authorize_interactively(&name, &url, &credential_store_path, |auth_url| {
        let _ = tauri_plugin_opener::open_url(auth_url, None::<&str>);
    })
    .await
    .map_err(|e| format!("{e:#}"))?;

    let new_orchestrator = bootstrap(None, Overrides::default(), desktop_default_vault_path()).await.map_err(|e| format!("{e:#}"));
    *state.orchestrator.lock().unwrap() = new_orchestrator;
    Ok(())
}

/// Forgets a server's stored OAuth token (Settings' "Disconnect" button) — its next connection
/// attempt starts a fresh authorization instead of trying to reuse or refresh the old one.
#[tauri::command]
async fn mcp_oauth_disconnect(state: State<'_, AppState>, name: String) -> Result<(), String> {
    warden_core::tool::mcp_oauth::forget_credentials(&oauth_credential_store_path(&name)).await.map_err(|e| format!("{e:#}"))?;

    let new_orchestrator = bootstrap(None, Overrides::default(), desktop_default_vault_path()).await.map_err(|e| format!("{e:#}"));
    *state.orchestrator.lock().unwrap() = new_orchestrator;
    Ok(())
}

#[tauri::command]
fn list_conversations() -> Result<Vec<Conversation>, String> {
    let dir = default_conversations_dir().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    read_conversations(&dir).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn save_conversation(conversation: Conversation) -> Result<(), String> {
    let dir = default_conversations_dir().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    write_conversation(&dir, &conversation).map_err(|e| format!("{e:#}"))
}

/// Backs the "Usage" nav view — the same on-demand aggregation the `usage_stats` tool (`warden-
/// bootstrap`'s `usage.rs`) gives the model itself, read fresh from disk on every call rather than
/// cached, so it can never show a stale number after a new message is sent.
#[tauri::command]
fn usage_summary() -> Result<UsageSummary, String> {
    let dir = default_conversations_dir().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let conversations = read_conversations(&dir).map_err(|e| format!("{e:#}"))?;
    Ok(aggregate_usage(&conversations))
}

/// Where sync tracking state lives — same OS config dir as `config.toml`, just a different file
/// (`warden_sync::paths`), so a fallback matches the same spirit as `default_config_path`'s own
/// callers elsewhere in this file (an OS with no resolvable config dir is exotic enough that a
/// relative-path fallback is fine, never hit in practice).
fn sync_secrets_path() -> PathBuf {
    warden_sync::paths::default_sync_secrets_path().unwrap_or_else(|| PathBuf::from("sync_secrets.json"))
}

fn sync_manifest_path() -> PathBuf {
    warden_sync::paths::default_sync_manifest_path().unwrap_or_else(|| PathBuf::from("sync_manifest.json"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let orchestrator = tauri::async_runtime::block_on(bootstrap(None, Overrides::default(), desktop_default_vault_path()))
        .map_err(|e| format!("{e:#}"));
    let sync_config_path = default_config_path().unwrap_or_else(|| PathBuf::from("config.toml"));
    // Was hardcoded to `desktop_default_vault_path()` regardless of `config.vault_path` — a bug
    // (P61): the chat `Orchestrator` above already respects a custom vault path via `bootstrap()`,
    // but sync silently kept mirroring `~/Warden/vault` instead. Resolved the same way `bootstrap()`
    // does internally, via the now-shared `resolve_vault_path`; falls back to the old hardcoded
    // default if the config can't even be loaded (same resilience as the `Orchestrator` above —
    // the window still opens either way).
    let sync_vault_path = load_config(None)
        .map(|config| resolve_vault_path(&Overrides::default(), &config, desktop_default_vault_path()))
        .unwrap_or_else(|_| desktop_default_vault_path());
    let sync = warden_sync::SyncEngine::new(sync_vault_path, sync_config_path, sync_secrets_path(), sync_manifest_path());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { orchestrator: Mutex::new(orchestrator), recording: Mutex::new(None), sync, pending_push: Mutex::new(None) })
        .invoke_handler(tauri::generate_handler![
            send_message,
            read_attachment,
            transcribe_audio,
            synthesize_speech,
            start_recording,
            stop_recording,
            get_settings,
            save_settings,
            list_conversations,
            save_conversation,
            usage_summary,
            mcp_oauth_status,
            mcp_oauth_connect,
            mcp_oauth_disconnect,
            sync_cmds::sync_status,
            sync_cmds::sync_init,
            sync_cmds::sync_push_begin,
            sync_cmds::sync_push_await,
            sync_cmds::sync_pull,
            sync_cmds::pairing_start,
            sync_cmds::pairing_join,
            vault_cmds::list_vault_files,
            vault_cmds::read_vault_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
