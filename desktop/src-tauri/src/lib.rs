mod api_key_cmds;
mod approval;
mod git_sync_cmds;
mod lend_cmds;
mod node_cmds;
mod people_cmds;
mod qr;
mod recording;
mod server_cmds;
mod skills_cmds;
mod spend_cmds;
mod ssh_cmds;
mod sync_cmds;
mod task_cmds;
mod vault_cmds;
mod workspace_cmds;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use warden_bootstrap::settings::{
    check_active_provider, check_agents, check_combos, check_providers, config_version, default_models_by_kind, limits_into_config, prices_into_config,
};
use warden_bootstrap::{
    aggregate_usage, bootstrap, build_model_for, default_config_path, scope_to_agent, AgentExtras,
    default_conversations_dir, default_limit_configs, env_switches_limits_off, list_conversations as read_conversations, load_config, load_config_from_path,
    oauth_credential_store_path, resolve_generated_path, resolve_vault_path, save_config,
    append_messages, AgentConfig, AppendOptions, ConversationMessage, ApiKeys, ComboConfig, Conversation, FileConfig, GitSyncConfig, McpServerConfig,
    Overrides,
    Provider, ProviderConfig, UsageSummary,
};
use warden_core::model::{Attachment, Message};
use warden_core::orchestrator::Orchestrator;
use warden_core::spend::SpendContext;
use warden_server_protocol::protocol::{LimitSettingsDto, PriceSettingsDto};

struct AppState {
    /// Shared (`Arc`) with the embedded hub's settings host, which puts the orchestrator a web
    /// settings save builds here too (P78).
    orchestrator: Arc<Mutex<Result<Orchestrator, String>>>,
    /// Set between a `start_recording`/`stop_recording` pair (P28) — `None` otherwise.
    recording: Mutex<Option<recording::ActiveRecording>>,
    /// P37 — the vault+config sync engine (Arweave via TruthID). No `Mutex` around the engine
    /// itself: every method takes `&self` and does its own file I/O, nothing mutates in memory.
    sync: warden_sync::SyncEngine,
    /// Set by `sync_cmds::sync_push_begin`, taken by `sync_cmds::sync_push_await` — showing the
    /// QR and blocking on the TruthID phone are deliberately separate IPC calls.
    pending_push: Mutex<Option<warden_sync::push::BeginPushResult>>,
    /// Resolved once at startup, same as `sync`'s vault path below — the trusted root
    /// `open_generated_file` (P64) confines every path it's willing to open to, so a path a
    /// malicious/prompt-injected tool result claimed can never be opened outside of it.
    generated_files_root: PathBuf,
    /// Set while the desktop is embedding its own `warden-server` hub (Fase 9.1 follow-up, "virar
    /// o hub desta rede") — `None` when stopped. See `server_cmds.rs`.
    embedded_server: Mutex<Option<server_cmds::EmbeddedServerHandle>>,
    /// Actions waiting for the user's yes/no (P47 SSH, P46 `manage_agents`) — see `approval::TauriApprover`.
    approvals: Arc<approval::ApprovalBroker>,
    /// P61/P71 — the automatic vault sync: looped by `sync_cmds::spawn_auto_sync` and handed to the
    /// embedded hub, which answers the web's Sync screen with it, so the two never sync at once.
    sync_runner: Arc<warden_bootstrap::auto_sync::SyncRunner>,
    /// P97 — this computer lent to a hub as a node, while on. See `lend_cmds.rs`.
    lending: Mutex<Option<lend_cmds::LendHandle>>,
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

/// The inverse — media extracted from an MCP tool result (P64 frente 2) comes back from the
/// orchestrator as `Attachment` and needs to cross the IPC boundary as `AttachmentPayload`.
impl From<Attachment> for AttachmentPayload {
    fn from(a: Attachment) -> Self {
        AttachmentPayload { mime_type: a.mime_type, data: a.data }
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
    /// Media extracted from an MCP tool's result during this turn (P64 frente 2) — empty when no
    /// tool call produced any.
    attachments: Vec<AttachmentPayload>,
    /// Paths of files actually written to disk this turn (P64) — `generate_document`'s own
    /// result, or oversized MCP media spilled to disk. Feeds the chat UI's "Open" affordance via
    /// `open_generated_file`.
    generated_files: Vec<String>,
    /// The turn's provider failed and a reserve answered (P79) — the chat shows a discreet line
    /// above the answer. Not saved with the conversation.
    fallbacks: Vec<warden_server_protocol::protocol::ProviderFallbackDto>,
}

/// `agent_id`/`provider_id` are the per-conversation selectors (closes P3) — the frontend sends
/// an explicit `null` for either when not overriding, rather than omitting the key, so this stays
/// unambiguous `Option<String>` deserialization. Both only ever affect this one call: `history`
/// (built fresh from the conversation's stored messages each time) is what makes a mid-conversation
/// switch apply "from here on" without needing to touch anything already said.
#[tauri::command]
async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    history: Vec<ChatTurn>,
    content: String,
    attachments: Vec<AttachmentPayload>,
    agent_id: Option<String>,
    provider_id: Option<String>,
) -> Result<SendMessageResult, String> {
    // Spends as the desktop (P4) — set before scoping, so the agents this one delegates or writes to
    // spend the same way.
    let mut orchestrator = { state.orchestrator.lock().unwrap().clone() }?.with_spend_context(SpendContext::new("desktop"));
    let history: Vec<Message> = history.into_iter().map(Into::into).collect();
    let attachments: Vec<Attachment> = attachments.into_iter().map(Into::into).collect();

    let mut persona = None;
    if agent_id.is_some() || provider_id.is_some() {
        let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
        let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;

        if let Some(id) = &agent_id {
            // `message_agent` writes into the same conversations the sidebar lists, and tells the
            // window to reload them.
            let notify_app = app.clone();
            let extras = AgentExtras {
                conversations_dir: default_conversations_dir(),
                on_conversation_changed: Some(Arc::new(move |conversation_id: &str| {
                    let _ = notify_app.emit("conversations-changed", conversation_id.to_string());
                })),
            };
            if let Some(scoped) = scope_to_agent(&orchestrator, &config, Some(&path), id, extras) {
                persona = Some(scoped.persona);
                orchestrator = scoped.orchestrator;
            }
        }
        if let Some(id) = &provider_id {
            let model = build_model_for(&config, id, None).map_err(|e| format!("{e:#}"))?;
            orchestrator = orchestrator.with_model(model);
        }
    }

    // Tools that need a human "yes" (SSH hosts with `require_approval`, `manage_agents`) ask through
    // the window; every other channel has no approver and those actions are refused there.
    let orchestrator = orchestrator.with_approver(Arc::new(approval::TauriApprover { app, broker: state.approvals.clone() }));
    let outcome =
        orchestrator.handle_turn(&history, &content, attachments, persona.as_deref()).await.map_err(|e| format!("{e:#}"))?;
    Ok(SendMessageResult {
        content: outcome.content,
        usage: outcome.usage,
        attachments: outcome.attachments.into_iter().map(Into::into).collect(),
        generated_files: outcome.generated_files,
        fallbacks: outcome.fallbacks.into_iter().map(Into::into).collect(),
    })
}

/// Names of every tool the running orchestrator has, for the Settings screen's per-agent tool list
/// (P46). Empty while the orchestrator failed to start.
#[tauri::command]
fn list_tool_names(state: State<'_, AppState>) -> Vec<String> {
    let guard = state.orchestrator.lock().unwrap();
    guard.as_ref().map(|o| o.tools().iter().map(|t| t.spec().name).collect()).unwrap_or_default()
}

/// Opens a file `generate_document`/oversized MCP media (P64) wrote to disk with the OS default
/// app for it, from the chat UI's "Open" affordance. Canonicalizes both `path` and the trusted
/// `generated_files_root` and refuses anything outside it — defense in depth on top of
/// `generate_document`'s own `filename` validation, since this command is what actually turns a
/// path a tool claimed into a one-click action on the user's filesystem.
#[tauri::command]
fn open_generated_file(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let root = std::fs::canonicalize(&state.generated_files_root).map_err(|e| format!("{e:#}"))?;
    let target = std::fs::canonicalize(&path).map_err(|e| format!("{e:#}"))?;
    if !target.starts_with(&root) {
        return Err("refusing to open a file outside the generated-files directory".to_string());
    }
    tauri_plugin_opener::open_path(target, None::<&str>).map_err(|e| format!("{e:#}"))
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
    // `stop_recording` always produces `audio/wav` (native capture, P28).
    let filename = warden_core::transcribe::audio_filename_for_mime_type(&audio.mime_type);

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
    /// Kind "node" only (P93): the node's device id.
    #[serde(default)]
    node: String,
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
    /// Opt-in (P46) for the `manage_agents` tool — see `AgentConfig::can_manage_agents`.
    #[serde(default)]
    can_manage_agents: bool,
    /// Opt-in (P46) for the `message_agent` tool — see `AgentConfig::can_message_agents`.
    #[serde(default)]
    can_message_agents: bool,
    /// Opt-in (P92) for the `manage_tasks` tool — see `AgentConfig::can_manage_tasks`.
    #[serde(default)]
    can_manage_tasks: bool,
    /// Tool isolation (P46) — see `AgentConfig::allowed_tools`. `None` (JSON `null`) = every tool.
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
    /// P84 — the people this agent is shared with (`"*"` = everyone) — see `AgentConfig::shared_with`.
    #[serde(default)]
    shared_with: Vec<String>,
}

/// IPC shape for `GitSyncConfig` (P63/P71 Settings UI) — same "dedicated payload struct for
/// `camelCase` field names" reasoning as `ProviderPayload`. All-or-nothing: either both
/// fields are filled in (parses to `Some(GitSyncConfig)`) or the whole section is left blank
/// (`None`) — `save_settings` rejects anything in between before it ever reaches disk.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GitSyncConfigPayload {
    remote_url: String,
    token: String,
}

impl From<GitSyncConfig> for GitSyncConfigPayload {
    fn from(c: GitSyncConfig) -> Self {
        GitSyncConfigPayload { remote_url: c.remote_url, token: c.token }
    }
}

/// What the settings screen reads.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsSnapshot {
    providers: Vec<ProviderPayload>,
    active_provider: String,
    vault_path: String,
    /// Where `generate_document` and oversized MCP media (P64/P66) get written — empty string
    /// means "unset", same convention as `vault_path`, resolving at bootstrap time to a sibling
    /// of the vault path (see `resolve_generated_path`).
    generated_path: String,
    tavily_key: String,
    /// OpenAI API key for Whisper transcription (P28 part 2) — dedicated, independent of which
    /// provider is active for chat. Same "not set" = empty string convention as `tavily_key`.
    whisper_key: String,
    enable_shell: bool,
    /// Default model per provider kind, keyed by the same string the frontend uses for `kind`
    /// (`"gemini"`/`"openai"`/`"anthropic"`) — shown as the Model field's placeholder. No entry
    /// for `openai_compatible`, which has no universal default (see `default_model_for`).
    default_models: std::collections::BTreeMap<String, String>,
    /// External MCP servers (Phase 5.2/P25) — `McpServerConfig`'s own fields, for either
    /// transport (`name`/`command`/`args`/`env` for stdio, `name`/`url`/`headers` for HTTP), are
    /// already single-word, so the untagged enum round-trips over IPC as-is with no dedicated
    /// payload type (unlike `ProviderPayload`, which needed one for the `camelCase` API key
    /// field names).
    mcp_servers: Vec<McpServerConfig>,
    /// The agent registry (closes P3) — named personas a conversation can pick, alongside its
    /// model.
    agents: Vec<AgentPayload>,
    /// Connection details for the git sync backend (P63/P71) — `None` until the user fills in the
    /// form on the "Sync via Git" section. The transport the Sync screen's manual push/pull and the
    /// auto-sync loop use.
    git_sync: Option<GitSyncConfigPayload>,
    /// Named routing combos (P90) — `ComboConfig` as is: its fields are already single words.
    combos: Vec<ComboConfig>,
    /// SSH servers the AI can run commands on (P47) — see `ssh_cmds::SshHostPayload`.
    ssh_hosts: Vec<ssh_cmds::SshHostPayload>,
    /// The spending limits in `config.toml` (P4). `None` = no `[[limits]]` at all, which means the
    /// built-in safety net is in force (`default_limits`); `Some([])` = every limit switched off.
    /// The two are different on purpose, so they are different here too.
    limits: Option<Vec<LimitSettingsDto>>,
    /// The safety net as editable entries, for "customize" to start from — the numbers live in
    /// `warden_bootstrap::spend`, not in the frontend.
    default_limits: Vec<LimitSettingsDto>,
    /// `WARDEN_SPEND_LIMITS=off` in the environment beats whatever the file says; the screen says so.
    limits_disabled_by_env: bool,
    /// What each model charges per million tokens — nothing is built in.
    prices: Vec<PriceSettingsDto>,
    /// The config file's version (P78), sent back on save: the hub's web settings write the same
    /// file, and a save over a change made there must be refused, not silently undo it.
    version: String,
}

#[derive(Deserialize)]
struct SettingsFormPayload {
    version: String,
    providers: Vec<ProviderPayload>,
    active_provider: String,
    vault_path: String,
    generated_path: String,
    tavily_key: String,
    whisper_key: String,
    enable_shell: bool,
    mcp_servers: Vec<McpServerConfig>,
    agents: Vec<AgentPayload>,
    git_sync: Option<GitSyncConfigPayload>,
    combos: Vec<ComboConfig>,
    ssh_hosts: Vec<ssh_cmds::SshHostPayload>,
    // No `#[serde(default)]` on these two: a form that forgot to send them must fail loudly, not
    // read as "no limits configured" and quietly swap the user's own limits for the safety net.
    limits: Option<Vec<LimitSettingsDto>>,
    prices: Vec<PriceSettingsDto>,
}

#[tauri::command]
fn get_settings() -> Result<SettingsSnapshot, String> {
    let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let version = config_version(&path).map_err(|e| format!("{e:#}"))?;
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
                node: p.node.unwrap_or_default(),
            })
            .collect(),
        active_provider: config.active_provider.unwrap_or_default(),
        vault_path: config.vault_path.unwrap_or_default(),
        generated_path: config.generated_path.unwrap_or_default(),
        tavily_key: config.api_keys.tavily.unwrap_or_default(),
        whisper_key: config.api_keys.whisper.unwrap_or_default(),
        enable_shell: config.enable_shell.unwrap_or(false),
        default_models: default_models_by_kind(),
        mcp_servers: config.mcp_servers,
        // P84 — members' own agents are theirs: not on this screen, and kept as they are on save.
        agents: config
            .agents
            .into_iter()
            .filter(|a| a.owner.is_none())
            .map(|a| AgentPayload {
                id: a.id,
                persona: a.persona,
                provider_id: a.provider_id.unwrap_or_default(),
                can_delegate_to_agents: a.can_delegate_to_agents,
                can_manage_agents: a.can_manage_agents,
                can_message_agents: a.can_message_agents,
                can_manage_tasks: a.can_manage_tasks,
                allowed_tools: a.allowed_tools,
                shared_with: a.shared_with,
            })
            .collect(),
        git_sync: config.git_sync.map(GitSyncConfigPayload::from),
        combos: config.combos,
        ssh_hosts: config.ssh_hosts.into_iter().map(Into::into).collect(),
        limits: config.limits.map(|l| l.into_iter().map(Into::into).collect()),
        default_limits: default_limit_configs().into_iter().map(Into::into).collect(),
        limits_disabled_by_env: env_switches_limits_off(std::env::var("WARDEN_SPEND_LIMITS").ok().as_deref()),
        prices: config.prices.into_iter().map(Into::into).collect(),
        version,
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
    // P78: the hub's web settings write this same file. A form loaded before such a save would
    // otherwise put the old values (keys included) straight back.
    if config_version(&path).map_err(|e| format!("{e:#}"))? != payload.version {
        return Err("the settings changed since this screen loaded them (from the web settings or by hand) — reopen Settings to see them".to_string());
    }
    let existing = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;

    let providers = check_providers(
        payload
            .providers
            .into_iter()
            .map(|p| ProviderConfig { id: p.id, kind: p.kind, api_key: Some(p.api_key), base_url: Some(p.base_url), model: Some(p.model), node: Some(p.node) })
            .collect(),
    )?;

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

    let combos = check_combos(payload.combos, &providers)?;
    let agents = check_agents(
        payload
            .agents
            .into_iter()
            .map(|a| AgentConfig {
                id: a.id,
                persona: a.persona,
                provider_id: Some(a.provider_id),
                can_delegate_to_agents: a.can_delegate_to_agents,
                can_manage_agents: a.can_manage_agents,
                can_message_agents: a.can_message_agents,
                can_manage_tasks: a.can_manage_tasks,
                allowed_tools: a.allowed_tools,
                owner: None,
                shared_with: warden_bootstrap::users::clean_shares(a.shared_with, &existing.users),
            })
            .chain(existing.agents.iter().filter(|a| a.owner.is_some()).cloned())
            .collect(),
        &providers,
        &combos,
    )?;

    let ssh_hosts = ssh_cmds::hosts_into_config(payload.ssh_hosts, &agents)?;

    let limits = payload.limits.map(limits_into_config).transpose()?;
    let prices = prices_into_config(payload.prices)?;

    let active_provider = check_active_provider(&payload.active_provider, &providers, &combos)?;

    // All-or-nothing (P63/P71): a URL without a token (or the reverse) can't sync anything, so it's
    // rejected here rather than silently written half-formed.
    let git_sync = match payload.git_sync {
        Some(g) => {
            let remote_url = g.remote_url.trim().to_string();
            let token = g.token.trim().to_string();
            match (remote_url.is_empty(), token.is_empty()) {
                (true, true) => None,
                (false, false) => Some(GitSyncConfig { remote_url, token }),
                _ => return Err("git sync fields (remote URL and token) must be filled in together, or left entirely blank".to_string()),
            }
        }
        None => None,
    };

    let config = FileConfig {
        // The legacy single-provider fields are only ever read as a fallback when `providers`
        // is empty (see `resolve_model_provider` in warden-bootstrap) — once this screen has
        // saved at least once, the registry below is authoritative, so clear them instead of
        // leaving stale duplicate secrets sitting in the file.
        provider: None,
        model: None,
        vault_path: non_empty(payload.vault_path),
        generated_path: non_empty(payload.generated_path),
        enable_shell: Some(payload.enable_shell),
        // No Settings-screen UI yet (P46, config.toml/env-only advanced knobs: `delegate_max_depth`, `max_delegated_calls`,
        // `max_parallel_jobs`) — carry forward whatever was on disk instead of wiping it, same reasoning as `telegram_bot_token` above.
        delegate_max_depth: existing.delegate_max_depth,
        max_delegated_calls: existing.max_delegated_calls,
        max_parallel_jobs: existing.max_parallel_jobs,
        // Spending limits and prices (P4): edited on the Settings screen (`spend_cmds`).
        limits,
        prices,
        api_keys: ApiKeys {
            gemini: None,
            openai: None,
            tavily: non_empty(payload.tavily_key),
            telegram_bot_token: existing.api_keys.telegram_bot_token,
            whisper: non_empty(payload.whisper_key),
        },
        providers,
        active_provider,
        combos,
        legacy_fallback_providers: Vec::new(),
        mcp_servers,
        agents,
        // Read and dropped (Sessão 105): the vault always lives locally now.
        legacy_storage_provider: None,
        legacy_remote_node: None,
        git_sync,
        // Owned by `server_cmds::save_embedded_server_config`/`start_embedded_server`, not this
        // general Settings save — carry forward unchanged, same reasoning as `git_sync` above.
        embedded_server: existing.embedded_server,
        ssh_hosts,
        // Scheduled tasks (P92) have no Settings screen yet — carry them forward.
        tasks: existing.tasks,
        // Nodes (P93) are edited on the Workspace screen (`node_cmds`), not this form.
        nodes: existing.nodes,
        // People (P84) are managed on the Workspace screen and the hub, not this form.
        users: existing.users,
        // Removed members keep their wrapped key here (P84 fatia 4): dropping it would lose their data.
        removed_users: existing.removed_users,
        spaces: existing.spaces,
    };

    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;

    reload_orchestrator(&state).await;
    Ok(())
}

/// Rebuilds the orchestrator from the config file for the desktop's chat and, when the embedded
/// hub is running, for the hub too (P78) — before this, the hub kept the orchestrator it started
/// with until it was restarted. A config the orchestrator can't start with leaves the hub on its
/// current one; the desktop shows the error in its chat.
async fn reload_orchestrator(state: &AppState) {
    let new_orchestrator = bootstrap(None, Overrides::default(), desktop_default_vault_path()).await.map_err(|e| format!("{e:#}"));
    if let (Ok(orchestrator), Some(hub)) = (&new_orchestrator, state.embedded_server.lock().unwrap().as_ref()) {
        hub.orchestrator.replace(orchestrator.clone());
    }
    *state.orchestrator.lock().unwrap() = new_orchestrator;
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

    reload_orchestrator(&state).await;
    Ok(())
}

/// Forgets a server's stored OAuth token (Settings' "Disconnect" button) — its next connection
/// attempt starts a fresh authorization instead of trying to reuse or refresh the old one.
#[tauri::command]
async fn mcp_oauth_disconnect(state: State<'_, AppState>, name: String) -> Result<(), String> {
    warden_core::tool::mcp_oauth::forget_credentials(&oauth_credential_store_path(&name)).await.map_err(|e| format!("{e:#}"))?;

    reload_orchestrator(&state).await;
    Ok(())
}

#[tauri::command]
fn list_conversations() -> Result<Vec<Conversation>, String> {
    let dir = default_conversations_dir().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    read_conversations(&dir).map_err(|e| format!("{e:#}"))
}

/// Adds messages to a conversation (creating it on the first one) and returns it as saved (P87).
/// The screen used to save the whole conversation it held, which erased anything another writer
/// added meanwhile — an agent answering a `message_agent` note in the "A → B" conversation, or the
/// CLI leaving one. Appending re-reads the file under the bootstrap's write lock instead.
#[tauri::command]
fn append_conversation_messages(
    conversation_id: String,
    messages: Vec<ConversationMessage>,
    title_seed: String,
    agent_id: Option<String>,
    provider_id: Option<String>,
) -> Result<Conversation, String> {
    let dir = default_conversations_dir().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let options = AppendOptions { title_seed: &title_seed, agent_id: agent_id.as_deref(), provider_id: Some(provider_id.as_deref()), create: true };
    append_messages(&dir, &conversation_id, options, messages)
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| "the conversation could not be created".to_string())
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
    // Same resolution `bootstrap()` uses internally for `generate_document`'s directory (P64) —
    // computed independently here (rather than threaded out of `bootstrap()`) because
    // `open_generated_file` needs it outside of any chat turn, at app startup. Falls back to the
    // exact same "sibling of the vault path" formula `resolve_generated_path` uses when there's
    // no config override, for the rare case the config can't even be loaded.
    let generated_files_root = load_config(None)
        .map(|config| resolve_generated_path(&config, &sync_vault_path))
        .unwrap_or_else(|_| sync_vault_path.parent().unwrap_or(std::path::Path::new(".")).join("generated"));
    let sync_runner = Arc::new(warden_bootstrap::auto_sync::SyncRunner::with_default_paths(sync_vault_path.clone(), sync_config_path.clone()));
    let sync = warden_sync::SyncEngine::new(sync_vault_path, sync_config_path, sync_secrets_path(), sync_manifest_path());

    let app_state = AppState {
        orchestrator: Arc::new(Mutex::new(orchestrator)),
        recording: Mutex::new(None),
        sync,
        pending_push: Mutex::new(None),
        generated_files_root,
        embedded_server: Mutex::new(None),
        approvals: Arc::new(approval::ApprovalBroker::default()),
        sync_runner: sync_runner.clone(),
        lending: Mutex::new(None),
    };

    // Fase 9.1 follow-up ("virar o hub desta rede") — a previously-enabled embedded server comes
    // back up on every launch, same always-on-service expectation as Jellyfin/similar self-hosted
    // apps; the operator flips it off explicitly (`stop_embedded_server`) rather than it silently
    // needing to be turned back on by hand every time.
    if let Ok(config) = load_config(None) {
        if let Some(server_config) = config.embedded_server.filter(|c| c.enabled) {
            match tauri::async_runtime::block_on(server_cmds::start_embedded_server_inner(&app_state, &server_config)) {
                Ok(handle) => {
                    eprintln!("desktop: embedded server auto-started on {}", handle.bound_addr);
                    *app_state.embedded_server.lock().unwrap() = Some(handle);
                }
                Err(err) => eprintln!("desktop: failed to auto-start the embedded server: {err:#}"),
            }
        }
    }
    // P97 — same for this computer lent to a hub: on at close, on again at launch. After the
    // embedded hub, so the "that's your own hub" check sees its port.
    lend_cmds::restore_lending(&app_state);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // P71 — pulls (and, for the git backend, pushes) the vault automatically every few
            // minutes instead of requiring a manual click, for as long as the app stays open. See
            // `sync_cmds::spawn_auto_sync`'s own doc comment for what this does and doesn't cover.
            sync_cmds::spawn_auto_sync(app.handle().clone(), sync_runner);
            Ok(())
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            send_message,
            list_tool_names,
            approval::resolve_approval,
            open_generated_file,
            read_attachment,
            transcribe_audio,
            synthesize_speech,
            start_recording,
            stop_recording,
            get_settings,
            save_settings,
            list_conversations,
            append_conversation_messages,
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
            git_sync_cmds::git_sync_configured,
            git_sync_cmds::git_sync_push,
            git_sync_cmds::git_sync_pull,
            vault_cmds::list_vault_files,
            spend_cmds::spend_status,
            spend_cmds::extend_spend_limit,
            vault_cmds::read_vault_note,
            vault_cmds::save_vault_note,
            vault_cmds::delete_vault_note,
            vault_cmds::search_vault,
            ssh_cmds::test_ssh_host,
            skills_cmds::list_skills,
            skills_cmds::save_skill,
            skills_cmds::delete_skill,
            skills_cmds::list_skill_files,
            skills_cmds::read_skill_attachment,
            skills_cmds::save_skill_attachment,
            skills_cmds::delete_skill_attachment,
            skills_cmds::generate_skill_draft,
            api_key_cmds::list_api_keys,
            api_key_cmds::create_api_key,
            api_key_cmds::revoke_api_key,
            node_cmds::list_nodes,
            node_cmds::save_node_access,
            people_cmds::list_people,
            people_cmds::add_person,
            people_cmds::rename_person,
            people_cmds::reset_person_password,
            people_cmds::set_person_tools,
            people_cmds::remove_person,
            people_cmds::list_shared_spaces,
            people_cmds::save_shared_space,
            people_cmds::remove_shared_space,
            lend_cmds::get_lend_status,
            lend_cmds::lend_options,
            lend_cmds::start_lending,
            lend_cmds::stop_lending,
            task_cmds::list_tasks,
            task_cmds::save_task,
            task_cmds::set_task_enabled_cmd,
            task_cmds::delete_task,
            task_cmds::run_task_now,
            task_cmds::task_history,
            task_cmds::set_run_tasks_here,
            workspace_cmds::list_paired_devices,
            workspace_cmds::approve_paired_device,
            workspace_cmds::revoke_paired_device,
            workspace_cmds::get_hub_pairing_config,
            workspace_cmds::save_hub_pairing_config,
            workspace_cmds::hub_pairing_qr_svg,
            workspace_cmds::discover_hubs,
            server_cmds::get_embedded_server_config,
            server_cmds::generate_embedded_server_auth_key,
            server_cmds::save_embedded_server_config,
            server_cmds::start_embedded_server,
            server_cmds::stop_embedded_server,
            server_cmds::embedded_server_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
