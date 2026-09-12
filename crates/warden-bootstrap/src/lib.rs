//! Shared bootstrap logic for every Warden channel (CLI, desktop, ...): load the TOML config
//! file, resolve provider/model/vault/API keys with a consistent precedence, and build a
//! ready-to-use `Orchestrator`. Kept out of `warden-core` on purpose — config-file/env-var
//! sourcing is an application-bootstrap concern specific to standalone deployments, not
//! something the model-agnostic engine itself should know about.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use warden_core::memory::{FIXED_VAULT_FILES, Vault};
use warden_core::model::anthropic::AnthropicProvider;
use warden_core::model::gemini::GeminiProvider;
use warden_core::model::openai::OpenAiProvider;
use warden_core::model::{Attachment, Message, ModelProvider, Usage};
use warden_core::orchestrator::{MessageOutcome, Orchestrator};
use warden_core::tool::delegate::DelegateTool;
use warden_core::tool::delegate_to_agent::{DelegateToAgentTool, NamedSubAgent};
use warden_core::tool::document::GenerateDocumentTool;
use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
use warden_core::tool::mcp::McpToolProvider;
use warden_core::tool::shell::ShellTool;
use warden_core::tool::{Tool, ToolProvider};

pub mod usage;
pub use usage::{aggregate_usage, UsageByKey, UsageStatsTool, UsageSummary};

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Gemini,
    Openai,
    Anthropic,
    /// Any other server that speaks the OpenAI chat-completions wire format — Ollama
    /// (local, no real key needed), OpenRouter, Groq, DeepSeek, etc. `ProviderConfig::base_url`
    /// is required for this kind; there's no single sensible default endpoint.
    OpenaiCompatible,
}

/// Where the vault's memory actually lives (P61) — selects the `warden_core::storage::StorageProvider`
/// `build_storage_provider` constructs. Unlike `Provider` above, this isn't a registry (there's
/// only ever one active storage backend per install, not several pre-configured ones to switch
/// between) — a single config field is enough. `RemoteNode`/`ManagedCloud` are placeholders for
/// v2/v3 (see `PENDING.md` P61) — `build_storage_provider` errors clearly if either is selected,
/// since neither has an implementation yet.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageProviderKind {
    /// Free, local disk — the absolute default, and what every install already does today
    /// (`Vault` itself, before this abstraction existed).
    Local,
    /// Paid-by-subscription, backed by TruthID/Arweave (`warden-sync`'s `SyncEngine`) — the only
    /// kind depending on TruthID. See `warden_sync::DecentralizedVaultProvider`'s doc comment for
    /// what it does and doesn't do yet.
    DecentralizedVault,
    /// v2, not implemented yet — another machine the user owns, via the node network (Fase 9).
    RemoteNode,
    /// v3, not implemented yet — traditional hosted infra, paid to Fabio, no Web3.
    ManagedCloud,
}

/// One configured model provider (Sessão 35's provider registry) — the desktop Settings screen
/// lets the user add/edit/delete any number of these, each independently selectable as the
/// active one. Kept as a flat list rather than a map so ordering is stable for display and `id`
/// collisions are the user's problem to fix (mirrors `McpServerConfig`'s shape/spirit).
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// User-chosen, unique among `providers` — referenced by `FileConfig::active_provider`.
    pub id: String,
    pub kind: Provider,
    pub api_key: Option<String>,
    /// Only meaningful (and required) for `Provider::OpenaiCompatible`.
    pub base_url: Option<String>,
    /// Falls back to `default_model_for(kind)` when unset — `None` for `OpenaiCompatible`,
    /// which has no universal default (depends entirely on what's hosted there).
    pub model: Option<String>,
}

/// One named agent (a persona a conversation can pick, alongside its model) — closes P3
/// (system-prompt/persona format). Same "flat list, `id` doubles as display name, collisions are
/// the user's problem" shape as `ProviderConfig`, edited the same way from the desktop Settings
/// screen.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// User-chosen, unique among `agents` — referenced by a `Conversation`'s `agent_id`.
    pub id: String,
    /// Free text, sent verbatim as a system-prompt message ahead of the vault context — no
    /// structure/parsing imposed on it (the user's own explicit ask: just a text field describing
    /// personality/behavior).
    pub persona: String,
    /// This agent's default model, referencing a `providers` entry by id. `None` means "use
    /// whatever the conversation already has selected" — picking this agent in the desktop just
    /// pre-fills the model selector with this when set, it isn't enforced afterward.
    pub provider_id: Option<String>,
    /// Opt-in for P46's "chief" mechanism — only an agent with this set to `true` gets the
    /// `delegate_to_agent` tool attached to its turns (see `build_delegate_to_agent_tool`). No
    /// Settings/CLI UI to toggle it yet — hand-edit `config.toml`.
    /// `#[serde(default)]` (not `Option`) so every config.toml written before this field existed
    /// still parses — same reasoning as `McpServerConfig::Http::oauth`.
    #[serde(default)]
    pub can_delegate_to_agents: bool,
}

/// Config for `StorageProviderKind::RemoteNode` (P61, v2) — which `warden-server` hub both this
/// device and the one actually holding the vault connect through, and which of its registered
/// devices is the target. No Settings-screen UI yet (config.toml/env only, same posture as
/// `AgentConfig`/`delegate_max_depth`) — and there's no way to usefully fill this in yet either,
/// since the target-side "vault node agent" process this would point at doesn't exist.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoteNodeConfig {
    /// The `warden-server` hub both sides connect through, e.g. `"ws://100.x.x.x:7420"`.
    pub server_url: String,
    /// This device's own id when it connects to `server_url` as a client.
    pub device_id: String,
    pub device_name: String,
    /// Shared secret for `server_url`'s `Hello` handshake.
    pub auth_key: String,
    /// The *other* device (already connected to the same hub) that actually holds the vault.
    pub target_device_id: String,
}

/// Config for `warden_sync::GitSyncEngine` (P63, v1) — a self-hosted/remote git repo (Gitea,
/// GitHub, ...) as an alternative to Arweave/TruthID for syncing the vault, for whoever doesn't
/// want that dependency. HTTPS + token only in v1 (SSH/deploy-key is v2); the token is only ever
/// read here and passed to `git` as a per-invocation URL credential — `GitSyncEngine` never writes
/// it to disk. No Settings-screen UI yet (config.toml only), same posture `RemoteNodeConfig` had
/// before P61 v2.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GitSyncConfig {
    /// The bare repo URL, no credentials — e.g. `"https://gitea.example.com/user/vault.git"`.
    pub remote_url: String,
    /// Personal access token for `remote_url`'s HTTPS auth.
    pub token: String,
}

/// Config file shape (TOML). Every field is optional — overrides and env vars (for API keys)
/// always win over what's here, and the whole file is optional too.
#[derive(Deserialize, Serialize, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    /// Deprecated as of Sessão 35's provider registry (`providers`/`active_provider` below) —
    /// kept only so a `config.toml` written before that still parses instead of erroring on an
    /// unknown field. `bootstrap()` reads this only as a fallback when `providers` is empty; a
    /// save from the new desktop Settings UI always clears it back to `None`.
    pub provider: Option<Provider>,
    /// Same deprecation as `provider` above.
    pub model: Option<String>,
    pub vault_path: Option<String>,
    /// Where `generate_document` (P64 v1) writes deliverable files — deliberately separate from
    /// the memory vault (not synced, not part of `search`/`search_semantic`). `None` derives a
    /// default as a sibling of the resolved `vault_path` (`<vault_path>/../generated`), same
    /// human-browsable-folder convention `desktop_default_vault_path()` already uses for the
    /// vault itself. No UI/CLI flag yet — config.toml only, same posture `enable_shell` had before
    /// it grew a Settings toggle.
    pub generated_path: Option<String>,
    /// Opt-in gate for the `shell` tool (Phase 5.5) — off unless explicitly turned on, since it
    /// lets the model run arbitrary commands on this machine with no sandboxing.
    pub enable_shell: Option<bool>,
    /// Overrides how many levels deep a sub-agent spawned via `DelegateTool` can itself delegate
    /// further (P46's recursive delegation, Sessão 57). `None` keeps `DEFAULT_DELEGATE_MAX_DEPTH`;
    /// `WARDEN_DELEGATE_MAX_DEPTH` wins over this if set (same precedence as
    /// `enable_shell`/`WARDEN_ENABLE_SHELL`). No UI yet — config.toml/env only, and no upper clamp:
    /// raising this raises the worst-case model-call blowup documented on `DEFAULT_DELEGATE_MAX_DEPTH`
    /// (see `PENDING.md` P60), deliberately left to the user to weigh.
    pub delegate_max_depth: Option<u32>,
    #[serde(default)]
    pub api_keys: ApiKeys,
    /// The provider registry (Sessão 35). Empty means "not migrated to the registry yet" —
    /// `bootstrap()` falls back to `provider`/`api_keys.gemini`/`api_keys.openai` in that case.
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// `id` of the `providers` entry to use. Ignored (and unnecessary) while `providers` is
    /// empty and the legacy fallback is in play.
    pub active_provider: Option<String>,
    /// External MCP servers to connect to on startup (Phase 5.2) — empty by default, same
    /// "off unless configured" spirit as the shell tool. Each entry is spawned as a local child
    /// process (stdio transport, the standard for local MCP servers); whatever tools it
    /// advertises get registered alongside the built-in ones.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// The agent registry — named personas a conversation can pick, alongside its model (closes
    /// P3). Empty by default; unlike `providers`, there's no "active" one — a conversation with
    /// no `agent_id` just runs with no persona, the same behavior as before this existed.
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    /// Where the vault's memory lives (P61). `None` means "not migrated yet" — resolved to
    /// `StorageProviderKind::Local` by `resolve_storage_provider`, same as every install already
    /// implicitly was before this field existed. Has a desktop Settings screen (the "Storage"
    /// section) as of this session; `RemoteNode`/`ManagedCloud` are still shown there as "coming
    /// soon" and can't be selected through it.
    pub storage_provider: Option<StorageProviderKind>,
    /// Only meaningful when `storage_provider` is `RemoteNode` — see `RemoteNodeConfig`'s own doc
    /// comment for why there's no UI for this yet either.
    pub remote_node: Option<RemoteNodeConfig>,
    /// Sync via a remote git repo instead of Arweave/TruthID (P63) — `None` means this backend
    /// isn't configured; unrelated to `storage_provider`/`remote_node` above, which are about
    /// where the vault's *primary copy* lives, not how it's synced between devices.
    pub git_sync: Option<GitSyncConfig>,
}

/// One external MCP server to connect to (TOML: `[[mcp_servers]]`), over either transport `rmcp`
/// speaks client-side — a local process over stdio (the original, still the common case: every
/// other MCP client's config uses this same `command`/`args`/`env` shape, e.g. Claude Desktop's
/// `mcpServers`, so a config from elsewhere ports over close to verbatim) or a remote server over
/// streamable HTTP (added for PENDING.md P25 — some servers, e.g. Slack's official one, are
/// hosted-only and were unreachable before this).
///
/// `#[serde(untagged)]` rather than an explicit `transport` tag: it lets every `config.toml`
/// written before this (`command`/`args`/`env`, no tag at all) keep parsing unchanged — the
/// `Stdio` variant *is* that exact old shape. A new HTTP entry is distinguished purely by having
/// `url` instead of `command`, which is also the only field distinction the desktop Settings UI
/// needs to render one or the other.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum McpServerConfig {
    Stdio {
        /// Only used for logging/error messages — not sent to the server.
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: std::collections::HashMap<String, String>,
    },
    Http {
        /// Only used for logging/error messages — not sent to the server.
        name: String,
        url: String,
        /// Sent on every request — typically just `Authorization: Bearer <token>` for a server
        /// that authenticates that way (see `McpToolProvider::connect_http`'s doc comment for why
        /// this is a static header rather than a full OAuth client). Ignored when `oauth` is true.
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
        /// When true, connect via the OAuth flow (`tool/mcp_oauth.rs`, PENDING.md P26) instead of
        /// `headers` — discovery, Dynamic Client Registration, browser consent, token refresh,
        /// with the token persisted under `oauth_credential_store_path(name)`. Mutually exclusive
        /// with `headers` (a server picks one auth mechanism or the other, not both).
        #[serde(default)]
        oauth: bool,
    },
}

impl McpServerConfig {
    pub fn name(&self) -> &str {
        match self {
            McpServerConfig::Stdio { name, .. } | McpServerConfig::Http { name, .. } => name,
        }
    }
}

#[derive(Deserialize, Serialize, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApiKeys {
    pub gemini: Option<String>,
    pub openai: Option<String>,
    pub tavily: Option<String>,
    /// Bot token from @BotFather (Fase 2) — only read by `warden-telegram`, not by `bootstrap()`
    /// itself, since a Telegram bot token isn't a model/tool secret the orchestrator needs.
    pub telegram_bot_token: Option<String>,
    /// OpenAI API key covering both ends of voice (P28 parts 2 and 3) — dedicated, independent
    /// of the active chat provider (same "own key, own capability" shape as `tavily` above), so
    /// voice works no matter which of the 3 chat providers is active. Named after the first
    /// capability it enabled (Whisper transcription, Sessão 41); reused as-is for TTS (Sessão
    /// 42, `/v1/audio/speech`) rather than adding a second key, since both are OpenAI audio
    /// endpoints on the same account. Only read directly by desktop's `transcribe_audio`/
    /// `synthesize_speech` IPC commands, not by `bootstrap()` — neither is an orchestrator
    /// `Tool`, both run outside `handle_message` entirely (one before it, one after).
    pub whisper: Option<String>,
}

/// The model name used when neither an override nor the config file specify one. `None` for
/// `Provider::OpenaiCompatible` — there's no universal default across arbitrary OpenAI-compatible
/// servers (an Ollama model tag, an OpenRouter slug, ...), so that kind always requires an
/// explicit `model` in its `ProviderConfig`.
///
/// These go stale as providers retire old models — `gemini-2.5-flash` (the original default,
/// Sessão 1) started 404ing for new API keys as of Sessão 32 ("no longer available to new
/// users"), confirming the risk flagged in `SESSIONS.md` back then. If a default here starts
/// erroring again, check the provider's current model list before assuming it's a code bug.
pub fn default_model_for(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::Gemini => Some("gemini-3.5-flash"),
        Provider::Openai => Some("gpt-4o-mini"),
        Provider::Anthropic => Some("claude-sonnet-4-5"),
        Provider::OpenaiCompatible => None,
    }
}

/// Writes `config` as TOML to `path`, creating the parent directory if it doesn't exist yet
/// (the default OS config dir may never have been created before the first save from a
/// settings UI).
pub fn save_config(path: &Path, config: &FileConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory at {}", parent.display()))?;
    }
    let contents = toml::to_string_pretty(config).context("failed to serialize config")?;
    std::fs::write(path, contents).with_context(|| format!("failed to write config file at {}", path.display()))
}

pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("config.toml"))
}

/// Propagates a provider id rename to everything in `config` that referenced the old id —
/// `active_provider` and every agent's `provider_id` — so renaming a provider from a caller that
/// commits straight to disk (the CLI's `/models edit`, unlike the desktop's edit-then-Save form)
/// can't leave a dangling reference the way P32 did before the desktop's `SettingsView.tsx` grew
/// the same cascade in its local draft state.
pub fn rename_provider_cascade(config: &mut FileConfig, old_id: &str, new_id: &str) {
    if config.active_provider.as_deref() == Some(old_id) {
        config.active_provider = Some(new_id.to_string());
    }
    for agent in &mut config.agents {
        if agent.provider_id.as_deref() == Some(old_id) {
            agent.provider_id = Some(new_id.to_string());
        }
    }
}

/// Clears every reference to a provider id that's about to be removed from `config` — same
/// cascade as `rename_provider_cascade`, for the P33 case (deleting rather than renaming).
pub fn remove_provider_references(config: &mut FileConfig, removed_id: &str) {
    if config.active_provider.as_deref() == Some(removed_id) {
        config.active_provider = None;
    }
    for agent in &mut config.agents {
        if agent.provider_id.as_deref() == Some(removed_id) {
            agent.provider_id = None;
        }
    }
}

/// Where an OAuth-authenticated MCP server's persisted token lives (PENDING.md P26) — one JSON
/// file per server, keyed by name. Names outside `[a-zA-Z0-9_-]` are sanitized to `_`; two server
/// names that only differ by punctuation collide here, same "the user's problem to fix" stance
/// already taken for other name-keyed config (see `ProviderConfig`'s doc comment).
pub fn oauth_credential_store_path(server_name: &str) -> PathBuf {
    let sanitized: String = server_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("warden").join("mcp_oauth").join(format!("{sanitized}.json"))
}

/// Chat message roles as persisted to disk — mirrors the frontend's `ChatRole`
/// (`desktop/src/types.ts`), the only two roles ever shown in the chat UI.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub id: String,
    pub role: ChatRole,
    pub content: String,
    pub created_at: i64,
    /// Token usage for this message's model call(s), when the provider reported it (Fase 5.8).
    /// `#[serde(default)]` so conversations saved before this field existed still load.
    #[serde(default)]
    pub usage: Option<Usage>,
    /// Images attached to this turn (P28, user messages only). `#[serde(default)]` so
    /// conversations saved before this field existed still load.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// A whole conversation as persisted to disk — mirrors the frontend's `Conversation`
/// (`desktop/src/types.ts`) field for field, so a loaded value can be handed straight back to
/// the UI with no reshaping at the IPC boundary.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub messages: Vec<ConversationMessage>,
    pub created_at: i64,
    pub updated_at: i64,
    /// The agent/provider last selected for this conversation (the desktop's per-conversation
    /// selectors), so reopening it restores the same choice. `#[serde(default)]` so conversations
    /// saved before these existed still load — same retrocompatibility as `usage`/`attachments`
    /// on `ConversationMessage`. `None` means "no override": no persona, and whatever
    /// `active_provider` currently resolves to.
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
}

/// Conversations are opaque app data (unlike the human-browsable markdown vault), so — like
/// the config file — they live under the OS config dir, not the vault.
pub fn default_conversations_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("conversations"))
}

/// Writes `conversation` as pretty JSON, one file per conversation named by id — an overwrite
/// of the whole file each time, same as `save_config`, since a conversation is small and there's
/// no concurrent writer to race with. Creates the directory if it doesn't exist yet.
pub fn save_conversation(dir: &Path, conversation: &Conversation) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create conversations directory at {}", dir.display()))?;
    let path = dir.join(format!("{}.json", conversation.id));
    let contents = serde_json::to_string_pretty(conversation).context("failed to serialize conversation")?;
    std::fs::write(&path, contents).with_context(|| format!("failed to write conversation file at {}", path.display()))
}

/// Lists every persisted conversation, newest-updated first. A directory that doesn't exist yet
/// just means "no conversations saved" — not an error, mirroring `load_config_from_path`'s
/// non-required case. A file that fails to parse is skipped rather than failing the whole list,
/// so one corrupt conversation can't make every other one disappear from the sidebar.
pub fn list_conversations(dir: &Path) -> anyhow::Result<Vec<Conversation>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read conversations directory at {}", dir.display()))
        }
    };

    let mut conversations = Vec::new();
    for entry in entries {
        let path = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(conversation) = serde_json::from_str::<Conversation>(&contents) {
                conversations.push(conversation);
            }
        }
    }

    conversations.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
    Ok(conversations)
}

/// Loads a single conversation by id, or `None` if no file exists for it yet — the "one
/// conversation" counterpart to `list_conversations`' "missing directory means empty" case.
/// Channels that thread conversations by a stable external id (Telegram's `chat_id`, Fase 2;
/// WhatsApp's later) use this instead of `list_conversations`, which would mean reading every
/// other conversation's file on every incoming message.
pub fn load_conversation(dir: &Path, id: &str) -> anyhow::Result<Option<Conversation>> {
    let path = dir.join(format!("{id}.json"));
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let conversation = serde_json::from_str(&contents)
                .with_context(|| format!("failed to parse conversation file at {}", path.display()))?;
            Ok(Some(conversation))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read conversation file at {}", path.display())),
    }
}

/// Telegram conversations are kept in their own directory rather than mixed into
/// `default_conversations_dir()` — that one is what the desktop sidebar lists in full, and a
/// Telegram chat (keyed by numeric `chat_id`, no client-side title) isn't meant to show up
/// there. A unified cross-channel conversation view is a bigger product question left for later.
pub fn default_telegram_conversations_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("conversations-telegram"))
}

/// Same reasoning as `default_telegram_conversations_dir` — WhatsApp chats are keyed by JID, not
/// a client-side title, and shouldn't show up in the desktop sidebar's `default_conversations_dir`.
pub fn default_whatsapp_conversations_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("conversations-whatsapp"))
}

/// Same reasoning as `default_telegram_conversations_dir`/`default_whatsapp_conversations_dir` —
/// `warden-server` (Fase 7.3) keys conversations by `device_id`, not a client-side title, and
/// shouldn't show up in the desktop sidebar's `default_conversations_dir`.
pub fn default_server_conversations_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("conversations-server"))
}

/// Where `warden-server`'s persistent device pairing registry lives (Fase 9.3) — same
/// `dirs::config_dir()` base as `default_server_conversations_dir`, not overridable via CLI yet
/// (same posture that function already has).
pub fn default_server_devices_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("devices.json"))
}

/// What the desktop's Workspace screen embeds in the QR code a new client scans (Fase 9.7) — the
/// two fields a `RemoteNodeConfig`/mobile `ConnectionScreen` would otherwise need typed by hand:
/// which hub to connect to, and its shared secret. Deliberately its own tiny JSON file, not a
/// field on `FileConfig`: it's a Workspace-only concern (generating a QR), unrelated to the
/// providers/agents/mcp settings that file's Settings-screen form already covers, and — unlike
/// `RemoteNodeConfig` — it never gets read by `bootstrap()`/`build_storage_provider`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HubPairingConfig {
    /// The `warden-server` hub to embed in the QR, e.g. `"ws://192.168.1.10:7420"`.
    pub server_url: String,
    /// Shared secret for that hub's `Hello` handshake — same value the operator passed it via
    /// `WARDEN_SERVER_AUTH_KEY`/`--auth-key` when starting it.
    pub auth_key: String,
}

/// Where `HubPairingConfig` lives (Fase 9.7) — same `dirs::config_dir()` base as
/// `default_server_devices_path`, separate file since it's JSON (no reason to force it into TOML)
/// and has nothing to do with `devices.json`'s pairing *records*.
pub fn default_hub_pairing_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("hub_pairing.json"))
}

/// `None` when the file doesn't exist yet (the operator hasn't filled in the Workspace form) —
/// same "missing file is just an empty/default state, not an error" posture as
/// `device_registry.rs::load`.
pub fn load_hub_pairing_config(path: &Path) -> anyhow::Result<Option<HubPairingConfig>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(serde_json::from_str(&contents)?)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn save_hub_pairing_config(path: &Path, config: &HubPairingConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory at {}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(config).context("failed to serialize hub pairing config")?;
    std::fs::write(path, contents).with_context(|| format!("failed to write hub pairing config at {}", path.display()))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// A short, unique-enough id for a `ConversationMessage` — no `uuid` dependency needed just for
/// this, nanosecond-resolution timestamps are already how this crate's own tests get uniqueness
/// (see `temp_dir`/`temp_toml_path` below).
fn message_id() -> String {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos().to_string()
}

/// Same truncation the frontend's `titleFromMessage` (`desktop/src/App.tsx`) uses for a new
/// conversation's title: collapse whitespace, cut to 40 chars with an ellipsis.
fn title_from(content: &str) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 40 {
        format!("{}…", collapsed.chars().take(40).collect::<String>())
    } else {
        collapsed
    }
}

fn to_message(message: &ConversationMessage) -> Message {
    match message.role {
        ChatRole::User => Message::user_with_attachments(message.content.clone(), message.attachments.clone()),
        ChatRole::Assistant => Message::assistant(message.content.clone()),
    }
}

/// Runs one turn of a conversation threaded by a stable external id: loads its prior history (or
/// starts a fresh `Conversation`, titled from `title_seed`), calls `orchestrator.handle_message`,
/// appends both the user and assistant messages (with token usage on the assistant one, Fase 5.8)
/// and persists the result under `conversations_dir` before returning the model's answer. Shared
/// by every channel that threads conversations this way — Telegram today, WhatsApp later.
/// `warden-cli`'s REPL and the desktop app don't use this: the CLI has no persistence at all, and
/// desktop's frontend already does its own read/append/save around the IPC boundary.
pub async fn handle_turn(
    orchestrator: &Orchestrator,
    conversations_dir: &Path,
    conversation_id: &str,
    title_seed: &str,
    user_input: &str,
) -> anyhow::Result<MessageOutcome> {
    let mut conversation = load_conversation(conversations_dir, conversation_id)?.unwrap_or_else(|| {
        let now = now_millis();
        Conversation {
            id: conversation_id.to_string(),
            title: title_from(title_seed),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            agent_id: None,
            provider_id: None,
        }
    });

    let history: Vec<Message> = conversation.messages.iter().map(to_message).collect();
    let outcome = orchestrator.handle_message(&history, user_input).await?;

    conversation.messages.push(ConversationMessage {
        id: message_id(),
        role: ChatRole::User,
        content: user_input.to_string(),
        created_at: now_millis(),
        usage: None,
        attachments: Vec::new(),
    });
    conversation.messages.push(ConversationMessage {
        id: message_id(),
        role: ChatRole::Assistant,
        content: outcome.content.clone(),
        created_at: now_millis(),
        usage: outcome.usage,
        attachments: Vec::new(),
    });
    conversation.updated_at = now_millis();

    save_conversation(conversations_dir, &conversation)?;
    Ok(outcome)
}

/// Loads the config file. An explicit path that doesn't exist is an error (the caller asked
/// for it by name); the default OS config path is optional — most users won't have one yet.
pub fn load_config(explicit_path: Option<&str>) -> anyhow::Result<FileConfig> {
    match explicit_path {
        Some(p) => load_config_from_path(&PathBuf::from(p), true),
        None => match default_config_path() {
            Some(p) => load_config_from_path(&p, false),
            None => Ok(FileConfig::default()),
        },
    }
}

pub fn load_config_from_path(path: &Path, required: bool) -> anyhow::Result<FileConfig> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            toml::from_str(&contents).with_context(|| format!("failed to parse config file at {}", path.display()))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound && !required => Ok(FileConfig::default()),
        Err(err) => Err(err).with_context(|| format!("failed to read config file at {}", path.display())),
    }
}

/// Env var wins over the config file value — lets you override a saved key for one run
/// without editing the file.
pub fn resolve_secret(from_env: Option<String>, from_file: Option<String>) -> Option<String> {
    from_env.or(from_file)
}

/// Same precedence as `resolve_secret` (env wins over file), but for a boolean flag rather than
/// a secret string — used for the `shell` tool's opt-in gate.
pub fn resolve_flag(from_env: Option<String>, from_file: Option<bool>) -> bool {
    match from_env {
        Some(value) => matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes"),
        None => from_file.unwrap_or(false),
    }
}

/// Same env-wins-over-file precedence as `resolve_flag`, for `delegate_max_depth` (P46). An env
/// value that doesn't parse as a `u32` is treated the same as it not being set at all — falls
/// back to `from_file`, then `DEFAULT_DELEGATE_MAX_DEPTH` — rather than failing `bootstrap()`
/// outright over a malformed value on an advanced/optional knob.
pub fn resolve_delegate_max_depth(from_env: Option<String>, from_file: Option<u32>) -> u32 {
    from_env.and_then(|v| v.trim().parse().ok()).or(from_file).unwrap_or(DEFAULT_DELEGATE_MAX_DEPTH)
}

/// Env-wins-over-file precedence for `storage_provider` (P61), but unlike `resolve_flag`/
/// `resolve_delegate_max_depth` — both permissive about a malformed env value — an unrecognized
/// `WARDEN_STORAGE_PROVIDER` is a hard error: silently falling back to the file/default value would
/// mean a typo changes *where the user's memory lives* without any indication anything went wrong.
/// Defaults to `Local` when neither is set, matching what every install already did before this
/// field existed.
pub fn resolve_storage_provider(from_env: Option<String>, from_file: Option<StorageProviderKind>) -> anyhow::Result<StorageProviderKind> {
    match from_env {
        Some(value) => match value.trim().to_lowercase().as_str() {
            "local" => Ok(StorageProviderKind::Local),
            "decentralized_vault" => Ok(StorageProviderKind::DecentralizedVault),
            "remote_node" => Ok(StorageProviderKind::RemoteNode),
            "managed_cloud" => Ok(StorageProviderKind::ManagedCloud),
            other => Err(anyhow::anyhow!(
                "WARDEN_STORAGE_PROVIDER='{other}' is not a recognized storage provider — expected one of \
                 local, decentralized_vault, remote_node, managed_cloud"
            )),
        },
        None => Ok(from_file.unwrap_or(StorageProviderKind::Local)),
    }
}

/// Registers whatever tools an already-attempted MCP connection advertises, or logs a warning
/// and leaves `base_tools` untouched on failure — a misconfigured or unreachable server shouldn't
/// take down the whole orchestrator, same graceful-degradation spirit as a missing
/// `TAVILY_API_KEY`. Shared by the built-in Tavily connection and every user-configured entry in
/// `config.mcp_servers` (Phase 5.2/P25), regardless of which transport actually produced
/// `connect_result` — both need the exact same list→extend flow once connected.
async fn register_mcp_tools(base_tools: &mut Vec<Arc<dyn Tool>>, name: &str, connect_result: anyhow::Result<McpToolProvider>) {
    match connect_result {
        Ok(provider) => match provider.tools().await {
            Ok(tools) => base_tools.extend(tools),
            Err(err) => eprintln!("note: MCP server '{name}' connected but failed to list tools: {err:#}\n"),
        },
        Err(err) => eprintln!("note: MCP server '{name}' unavailable, skipping: {err:#}\n"),
    }
}

/// Per-channel overrides (CLI flags today; a desktop settings UI later — see PHASE.md 6.5).
#[derive(Default)]
pub struct Overrides {
    /// Legacy single-provider-kind override (the CLI's `--provider gemini|openai`) — only
    /// consulted by `resolve_model_provider`'s fallback path, when `config.providers` is empty.
    pub provider: Option<Provider>,
    /// Registry-aware override: the `id` of a `config.providers` entry to use instead of
    /// `config.active_provider`. No CLI flag sets this yet (registry management is desktop-only
    /// so far, see PENDING.md P22) — reserved for when one does.
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub vault_path: Option<String>,
}

/// Resolves the vault path with the same override-then-config-then-default precedence `bootstrap()`
/// itself uses — extracted out (P61) so callers that need a `Vault`/`StorageProvider` outside of
/// `bootstrap()` (e.g. the sync subsystem in CLI/desktop/mobile) don't have to re-derive this
/// independently. Previously duplicated by hand in `warden-cli`'s own `resolve_vault_path` and
/// missing entirely from the desktop's `SyncEngine` construction (which just hardcoded its own
/// default, ignoring `config.vault_path` — see `PENDING.md` P61).
pub fn resolve_vault_path(overrides: &Overrides, config: &FileConfig, default_vault_path: PathBuf) -> PathBuf {
    overrides.vault_path.clone().map(PathBuf::from).or_else(|| config.vault_path.clone().map(PathBuf::from)).unwrap_or(default_vault_path)
}

/// Resolves where `generate_document` (P64 v1) writes deliverable files. `config.generated_path`
/// wins when set; otherwise derives a sibling of the already-resolved `vault_path`
/// (`<vault_path>/../generated`) — the same human-browsable-folder convention
/// `desktop_default_vault_path()` uses for the vault itself (e.g. `~/Warden/vault` ->
/// `~/Warden/generated`), without `bootstrap()` needing a second caller-supplied default
/// parameter alongside `default_vault_path`.
pub fn resolve_generated_path(config: &FileConfig, resolved_vault_path: &Path) -> PathBuf {
    config
        .generated_path
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| resolved_vault_path.parent().unwrap_or_else(|| Path::new(".")).join("generated"))
}

/// Builds the `StorageProvider` for a resolved `StorageProviderKind` (P61) — the factory
/// `storage_provider`/`WARDEN_STORAGE_PROVIDER` select between. `async` (unlike every other
/// `build_*` helper in this file) because `RemoteNode` needs a real network round-trip
/// (`RemoteNodeProvider::connect`) to construct — `Local`/`DecentralizedVault` never touch
/// `.await` internally, but the signature has to accommodate the one variant that does.
/// `ManagedCloud` (v3, see `PENDING.md` P61) is still not implemented, and errors clearly rather
/// than silently falling back to `Local`. Not yet called from `bootstrap()` itself: `Orchestrator`
/// uses `Vault` directly for chat/memory (search, standing memory, ...) — none of which are
/// `StorageProvider` concerns — so this is additive machinery for the sync subsystem (and now
/// `RemoteNodeProvider`'s caller-side wiring) to adopt, not a replacement for how `Orchestrator`
/// already reads/writes the vault.
pub async fn build_storage_provider(
    kind: StorageProviderKind,
    vault: Arc<Vault>,
    remote_node: Option<&RemoteNodeConfig>,
) -> anyhow::Result<Arc<dyn warden_core::storage::StorageProvider>> {
    Ok(match kind {
        StorageProviderKind::Local => Arc::new(warden_core::storage::LocalFSProvider::new(vault)),
        StorageProviderKind::DecentralizedVault => Arc::new(warden_sync::DecentralizedVaultProvider::new(vault)),
        StorageProviderKind::RemoteNode => {
            let cfg = remote_node
                .ok_or_else(|| anyhow::anyhow!("storage_provider 'remote_node' requires a [remote_node] config section"))?;
            Arc::new(
                warden_server_protocol::RemoteNodeProvider::connect(
                    &cfg.server_url,
                    &cfg.device_id,
                    &cfg.device_name,
                    &cfg.auth_key,
                    cfg.target_device_id.clone(),
                )
                .await?,
            )
        }
        StorageProviderKind::ManagedCloud => {
            return Err(anyhow::anyhow!("storage_provider 'managed_cloud' is not implemented yet (planned for v3 — see PENDING.md P61)"));
        }
    })
}

/// Builds the `AuthProvider` for a resolved `StorageProviderKind` (P61) — mirrors
/// `build_storage_provider`'s per-kind dispatch, and shares its "additive machinery, nothing in
/// `bootstrap()` calls this yet" posture. Only `DecentralizedVault` needs real identity/payment
/// gating: `warden_sync::TruthIdAuthProvider`, backed by the pairing manifest already written by
/// `SyncEngine` — see its own doc comment for why `is_subscription_active` there is a pairing
/// check, not a real subscription check (no billing system exists anywhere in this codebase yet).
/// Every other kind gets `NoAuthProvider`: `Local` genuinely needs no identity to write to disk,
/// and `RemoteNode`/`ManagedCloud` have no `StorageProvider` implementation to gate in the first
/// place (`build_storage_provider` already errors on both before an `AuthProvider` would ever
/// matter) — never errors, unlike `build_storage_provider`, since `NoAuthProvider` is always a
/// valid (if trivial) answer for any kind.
pub fn build_auth_provider(kind: StorageProviderKind, manifest_path: PathBuf) -> Arc<dyn warden_core::storage::AuthProvider> {
    match kind {
        StorageProviderKind::DecentralizedVault => Arc::new(warden_sync::TruthIdAuthProvider::new(manifest_path)),
        StorageProviderKind::Local | StorageProviderKind::RemoteNode | StorageProviderKind::ManagedCloud => {
            Arc::new(warden_core::storage::NoAuthProvider)
        }
    }
}

/// Builds the one `ModelProvider` the orchestrator will use, from a resolved `ProviderConfig` —
/// shared by both the registry path and the legacy-fallback path in `resolve_model_provider`, and
/// (public since the per-conversation model selector) by the desktop's `send_message` to build a
/// one-off model for a `provider_id` override without re-running `bootstrap()`.
pub fn build_model_provider(provider: &ProviderConfig, model_override: Option<String>) -> anyhow::Result<Arc<dyn ModelProvider>> {
    let model = model_override.or_else(|| provider.model.clone()).or_else(|| default_model_for(provider.kind).map(str::to_string)).ok_or_else(|| {
        anyhow::anyhow!("provider '{}' ({:?}) has no model configured and no default exists for this kind", provider.id, provider.kind)
    })?;

    Ok(match provider.kind {
        Provider::Gemini => {
            let api_key = provider.api_key.clone().with_context(|| {
                format!(
                    "provider '{}' (gemini) has no API key configured — set GEMINI_API_KEY, or its api_key in config.toml \
                     / the desktop Settings screen (get a free key at https://aistudio.google.com/apikey)",
                    provider.id
                )
            })?;
            Arc::new(GeminiProvider::new(api_key, model))
        }
        Provider::Openai => {
            let api_key = provider.api_key.clone().with_context(|| {
                format!(
                    "provider '{}' (openai) has no API key configured — set OPENAI_API_KEY, or its api_key in config.toml \
                     / the desktop Settings screen",
                    provider.id
                )
            })?;
            Arc::new(OpenAiProvider::new(api_key, model))
        }
        Provider::Anthropic => {
            let api_key = provider
                .api_key
                .clone()
                .with_context(|| format!("provider '{}' (anthropic) has no API key configured — get one at https://console.anthropic.com", provider.id))?;
            Arc::new(AnthropicProvider::new(api_key, model))
        }
        Provider::OpenaiCompatible => {
            let base_url = provider
                .base_url
                .clone()
                .with_context(|| format!("provider '{}' (openai_compatible) has no base_url configured — e.g. http://localhost:11434/v1 for Ollama", provider.id))?;
            // Most local/self-hosted OpenAI-compatible servers (Ollama included) don't check the
            // key at all — an empty string is a valid "no key" for them.
            Arc::new(OpenAiProvider::with_base_url(provider.api_key.clone().unwrap_or_default(), model, base_url))
        }
    })
}

/// Builds the `delegate_to_agent` tool (P46's opt-in "chief" mechanism) from every configured
/// agent — call once per turn, after resolving which model/persona is active, only when the
/// active agent has `can_delegate_to_agents: true` (a caller attaches the result via
/// `Orchestrator::with_tool`). `orchestrator` should be the same orchestrator this turn is about
/// to use: each target agent gets a clone of it (`with_model` swapped in only if that agent's
/// `provider_id` differs), so every target inherits the exact same tool set/delegation depth as
/// everyone else, just its own persona/model layered on top.
///
/// An agent invoked as a *target* here never gets `delegate_to_agent` itself, even if its own
/// `can_delegate_to_agents` is `true` — that flag is only consulted by the caller for whichever
/// agent is the conversation's *active* one, never for a delegation target. Deliberate: avoids an
/// uncontrolled chief-of-chief chain without needing another depth limit (see `PENDING.md` P60,
/// same spirit as `DELEGATE_MAX_DEPTH`/`delegate_max_depth` for `delegate_task`).
///
/// Returns `None` when there's nothing to delegate to — no agents configured, or every one of
/// them failed to resolve a valid provider (logged via `eprintln!`, not fatal — one broken agent
/// shouldn't take down every other agent's ability to delegate).
pub fn build_delegate_to_agent_tool(config: &FileConfig, orchestrator: &Orchestrator) -> Option<Arc<dyn Tool>> {
    let mut targets = Vec::new();
    for agent in &config.agents {
        let target_orchestrator = match &agent.provider_id {
            Some(provider_id) => {
                let Some(provider) = config.providers.iter().find(|p| &p.id == provider_id) else {
                    eprintln!(
                        "note: agent '{}' references unknown provider '{provider_id}' — delegate_to_agent won't be able to reach it\n",
                        agent.id
                    );
                    continue;
                };
                match build_model_provider(provider, None) {
                    Ok(model) => orchestrator.with_model(model),
                    Err(err) => {
                        eprintln!("note: agent '{}' has an invalid provider — delegate_to_agent won't be able to reach it: {err:#}\n", agent.id);
                        continue;
                    }
                }
            }
            None => orchestrator.clone(),
        };
        let persona = (!agent.persona.trim().is_empty()).then(|| agent.persona.clone());
        targets.push(NamedSubAgent {
            id: agent.id.clone(),
            description: agent.persona.clone(),
            orchestrator: target_orchestrator,
            persona,
        });
    }
    (!targets.is_empty()).then(|| Arc::new(DelegateToAgentTool::new(targets)) as Arc<dyn Tool>)
}

/// Resolves which `ModelProvider` to build, in order: an explicit `overrides.provider_id` or
/// `config.active_provider` naming an entry in `config.providers` (the registry, Sessão 35);
/// otherwise, when `config.providers` is empty, a single provider synthesized from the older
/// `overrides.provider`/`config.provider`/`config.api_keys` fields plus the `GEMINI_API_KEY`/
/// `OPENAI_API_KEY` env vars — exactly the resolution `bootstrap()` did before the registry
/// existed, so a `config.toml` (or a bare env var, as most tests use) never on-boarded to the
/// registry keeps working unchanged.
fn resolve_model_provider(config: &FileConfig, overrides: &Overrides) -> anyhow::Result<Arc<dyn ModelProvider>> {
    if !config.providers.is_empty() {
        let active_id = overrides
            .provider_id
            .clone()
            .or_else(|| config.active_provider.clone())
            .ok_or_else(|| anyhow::anyhow!("providers are configured but no `active_provider` is set — pick one of: {}", config.providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ")))?;
        let provider = config
            .providers
            .iter()
            .find(|p| p.id == active_id)
            .ok_or_else(|| anyhow::anyhow!("active_provider '{active_id}' not found among configured providers"))?;
        return build_model_provider(provider, overrides.model.clone());
    }

    let kind = overrides.provider.or(config.provider).unwrap_or(Provider::Gemini);
    let (id, api_key_from_config) = match kind {
        Provider::Gemini => ("gemini", config.api_keys.gemini.clone()),
        Provider::Openai => ("openai", config.api_keys.openai.clone()),
        // Anthropic/OpenaiCompatible have no legacy single-slot field to fall back to — they only
        // exist via the registry, so picking one here with no `providers` configured is a no-op
        // that will fail clearly in `build_model_provider` (no api_key/base_url).
        Provider::Anthropic => ("anthropic", None),
        Provider::OpenaiCompatible => ("openai_compatible", None),
    };
    let env_var = match kind {
        Provider::Gemini => Some("GEMINI_API_KEY"),
        Provider::Openai => Some("OPENAI_API_KEY"),
        Provider::Anthropic | Provider::OpenaiCompatible => None,
    };
    let api_key = resolve_secret(env_var.and_then(|v| std::env::var(v).ok()), api_key_from_config);

    let synthesized = ProviderConfig { id: id.to_string(), kind, api_key, base_url: None, model: config.model.clone() };
    build_model_provider(&synthesized, overrides.model.clone())
}

/// Loads config, resolves provider/model/vault/API keys (override > config file > env >
/// built-in default), and builds a ready-to-use `Orchestrator` (including the delegate
/// sub-orchestrator). `default_vault_path` is the last-resort fallback when neither
/// `overrides.vault_path` nor the config file specify one — deliberately caller-supplied since
/// the right fallback differs per channel (a CLI user picks their own cwd; a GUI app can't).
pub async fn bootstrap(
    explicit_config_path: Option<&str>,
    overrides: Overrides,
    default_vault_path: PathBuf,
) -> anyhow::Result<Orchestrator> {
    let config = load_config(explicit_config_path)?;
    let model_provider = resolve_model_provider(&config, &overrides)?;

    let vault_path = resolve_vault_path(&overrides, &config, default_vault_path);
    let generated_path = resolve_generated_path(&config, &vault_path);

    let vault = Arc::new(Vault::new(vault_path));
    seed_default_vault_files(&vault);

    let mut base_tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ReadFileTool::new(vault.clone())),
        Arc::new(WriteFileTool::new(vault.clone())),
        Arc::new(GenerateDocumentTool::new(generated_path)),
        Arc::new(UsageStatsTool::new(default_conversations_dir())),
    ];

    match resolve_secret(std::env::var("TAVILY_API_KEY").ok(), config.api_keys.tavily) {
        Some(tavily_key) => {
            // Tavily's own MCP server (not a hand-rolled REST call) — gives search plus
            // extract/crawl/map for free, and doubles as real-world validation of the MCP
            // client (Phase 5.2) against a third-party server, not just the hand-written one
            // in warden-core's test suite. Trade-off accepted deliberately: this now needs
            // Node.js/npx on PATH at runtime, which a pure-Rust REST call didn't.
            let connect = McpToolProvider::connect_stdio(
                "tavily",
                "npx",
                &["-y".to_string(), "tavily-mcp".to_string()],
                &[("TAVILY_API_KEY".to_string(), tavily_key)],
            )
            .await;
            register_mcp_tools(&mut base_tools, "tavily", connect).await;
        }
        None => eprintln!(
            "note: TAVILY_API_KEY not set — web search (via tavily-mcp) disabled (get a free key at https://tavily.com)\n"
        ),
    }

    if resolve_flag(std::env::var("WARDEN_ENABLE_SHELL").ok(), config.enable_shell) {
        base_tools.push(Arc::new(ShellTool::new(vault.clone())));
    } else {
        eprintln!(
            "note: shell tool disabled — set WARDEN_ENABLE_SHELL=1 (or enable_shell = true in config.toml) to \
             enable it. It lets the model run arbitrary commands on this machine, with no sandboxing.\n"
        );
    }

    for server in &config.mcp_servers {
        let name = server.name();
        let connect = match server {
            McpServerConfig::Stdio { command, args, env, .. } => {
                let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                McpToolProvider::connect_stdio(name, command, args, &env).await
            }
            McpServerConfig::Http { url, oauth, .. } if *oauth => {
                warden_core::tool::mcp_oauth::connect_http_oauth(name, url, &oauth_credential_store_path(name)).await
            }
            McpServerConfig::Http { url, headers, .. } => {
                let headers: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                McpToolProvider::connect_http(name, url, &headers).await
            }
        };
        register_mcp_tools(&mut base_tools, name, connect).await;
    }

    let delegate_max_depth =
        resolve_delegate_max_depth(std::env::var("WARDEN_DELEGATE_MAX_DEPTH").ok(), config.delegate_max_depth);
    let orchestrator = build_delegating_orchestrator(model_provider, vault, &base_tools, delegate_max_depth);

    Ok(orchestrator)
}

/// Default for how many levels deep a sub-agent spawned via `DelegateTool` can itself delegate
/// further (P46 — "sub-agentes autônomos", core recursion piece), used when `delegate_max_depth`
/// isn't set in config.toml/env (see `resolve_delegate_max_depth`). No job queue or cost control
/// exists to bound a deep chain's total model calls (worst case is roughly
/// `MAX_TOOL_ITERATIONS ^ depth` if every single iteration at every level delegates), so a small
/// default is what keeps that worst case sane out of the box. Revisit alongside P4 (cost control)
/// if a use case needs deeper chains by default — see `PENDING.md` P60.
const DEFAULT_DELEGATE_MAX_DEPTH: u32 = 2;

/// Builds an `Orchestrator` with `base_tools` registered, plus — while `depth > 0` — a
/// `DelegateTool` wrapping another orchestrator built the same way one level shallower. The
/// terminal orchestrator (`depth == 0`) never gets a `DelegateTool`, so it never advertises
/// `delegate_task` in its tool specs — that's the actual stopping criterion (structural, not a
/// runtime check), see the doc comment on `DelegateTool` itself.
fn build_delegating_orchestrator(
    model: Arc<dyn ModelProvider>,
    vault: Arc<Vault>,
    base_tools: &[Arc<dyn Tool>],
    depth: u32,
) -> Orchestrator {
    let mut orchestrator = Orchestrator::new(model.clone(), vault.clone());
    for tool in base_tools {
        orchestrator.register_tool(tool.clone());
    }
    if depth > 0 {
        let sub = build_delegating_orchestrator(model, vault, base_tools, depth - 1);
        orchestrator.register_tool(Arc::new(DelegateTool::new(sub)));
    }
    orchestrator
}

/// Seeds the vault's "fixed/standard" memory files (P52 — `Vault::standing_memory`) with a
/// starter template the first time each one is used. Idempotent and non-destructive: only writes
/// a file that doesn't exist yet, so a vault restored via `warden-sync` from another device (which
/// already has these files, possibly edited) is never touched. Templates are short and in the
/// user's language (Portuguese, matching how they actually write vault notes) — a title plus one
/// line of guidance for both the user and the model on what belongs there.
fn seed_default_vault_files(vault: &Vault) {
    const TEMPLATES: [&str; 3] = [
        "# Perfil do usuário\n\n_Quem você é — nome, contexto, preferências gerais. Edite livremente; a IA também pode atualizar aqui quando aprender algo relevante sobre você._\n",
        "# Comportamento da IA\n\n_Como a IA deve agir e responder — regras gerais de conduta, válidas em qualquer agente/conversa (diferente da persona de um agente específico)._\n",
        "# Feedback e lições aprendidas\n\n_Correções e preferências de como você gosta de trabalhar, acumuladas com o tempo. A IA deve atualizar este arquivo quando aprender algo relevante._\n",
    ];
    for (name, template) in FIXED_VAULT_FILES.iter().zip(TEMPLATES) {
        if vault.read(name).is_err() {
            let _ = vault.write(name, template);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_toml_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-bootstrap-config-test-{name}-{}.toml",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    fn temp_json_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-bootstrap-hub-pairing-test-{name}-{}.json",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[test]
    fn hub_pairing_config_is_none_when_never_saved() {
        let path = temp_json_path("missing");
        assert_eq!(load_hub_pairing_config(&path).unwrap(), None);
    }

    #[test]
    fn hub_pairing_config_round_trips_through_save_and_load() {
        let path = temp_json_path("round-trip");
        let config = HubPairingConfig { server_url: "ws://192.168.1.10:7420".to_string(), auth_key: "secret".to_string() };

        save_hub_pairing_config(&path, &config).unwrap();
        let loaded = load_hub_pairing_config(&path).unwrap();

        assert_eq!(loaded, Some(config));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parses_a_valid_config_file() {
        let path = temp_toml_path("valid");
        std::fs::write(
            &path,
            r#"
provider = "openai"
model = "gpt-4o-mini"
vault_path = "/tmp/some-vault"

[api_keys]
gemini = "gk"
openai = "ok"
tavily = "tk"
"#,
        )
        .unwrap();

        let config = load_config(Some(path.to_str().unwrap())).unwrap();

        assert_eq!(config.provider, Some(Provider::Openai));
        assert_eq!(config.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(config.vault_path.as_deref(), Some("/tmp/some-vault"));
        assert_eq!(config.api_keys.gemini.as_deref(), Some("gk"));
        assert_eq!(config.api_keys.openai.as_deref(), Some("ok"));
        assert_eq!(config.api_keys.tavily.as_deref(), Some("tk"));
        assert!(config.mcp_servers.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parses_mcp_servers_from_toml() {
        let path = temp_toml_path("mcp-servers");
        std::fs::write(
            &path,
            r#"
[[mcp_servers]]
name = "anchor"
command = "npx"
args = ["-y", "@anchor/mcp-server"]

[mcp_servers.env]
ANCHOR_API_KEY = "secret"

[[mcp_servers]]
name = "no-args-server"
command = "some-mcp-server"
"#,
        )
        .unwrap();

        let config = load_config(Some(path.to_str().unwrap())).unwrap();

        assert_eq!(config.mcp_servers.len(), 2);
        assert_eq!(config.mcp_servers[0].name(), "anchor");
        match &config.mcp_servers[0] {
            McpServerConfig::Stdio { command, args, env, .. } => {
                assert_eq!(command, "npx");
                assert_eq!(args, &vec!["-y".to_string(), "@anchor/mcp-server".to_string()]);
                assert_eq!(env.get("ANCHOR_API_KEY").map(String::as_str), Some("secret"));
            }
            McpServerConfig::Http { .. } => panic!("expected a Stdio entry, got Http"),
        }
        assert_eq!(config.mcp_servers[1].name(), "no-args-server");
        match &config.mcp_servers[1] {
            McpServerConfig::Stdio { args, env, .. } => {
                assert!(args.is_empty());
                assert!(env.is_empty());
            }
            McpServerConfig::Http { .. } => panic!("expected a Stdio entry, got Http"),
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parses_http_mcp_server_from_toml() {
        // No `transport` tag needed (see the untagged-enum doc comment on `McpServerConfig`) —
        // a `url` field (instead of `command`) is what selects the `Http` variant.
        let path = temp_toml_path("mcp-http-server");
        std::fs::write(
            &path,
            r#"
[[mcp_servers]]
name = "slack"
url = "https://mcp.slack.com/mcp"

[mcp_servers.headers]
Authorization = "Bearer secret-token"
"#,
        )
        .unwrap();

        let config = load_config(Some(path.to_str().unwrap())).unwrap();

        assert_eq!(config.mcp_servers.len(), 1);
        assert_eq!(config.mcp_servers[0].name(), "slack");
        match &config.mcp_servers[0] {
            McpServerConfig::Http { url, headers, oauth, .. } => {
                assert_eq!(url, "https://mcp.slack.com/mcp");
                assert_eq!(headers.get("Authorization").map(String::as_str), Some("Bearer secret-token"));
                assert!(!oauth, "oauth should default to false when the field is absent from the TOML");
            }
            McpServerConfig::Stdio { .. } => panic!("expected an Http entry, got Stdio"),
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parses_oauth_http_mcp_server_from_toml() {
        let path = temp_toml_path("mcp-oauth-http-server");
        std::fs::write(
            &path,
            r#"
[[mcp_servers]]
name = "slack"
url = "https://mcp.slack.com/mcp"
oauth = true
"#,
        )
        .unwrap();

        let config = load_config(Some(path.to_str().unwrap())).unwrap();

        assert_eq!(config.mcp_servers.len(), 1);
        match &config.mcp_servers[0] {
            McpServerConfig::Http { url, headers, oauth, .. } => {
                assert_eq!(url, "https://mcp.slack.com/mcp");
                assert!(headers.is_empty());
                assert!(oauth);
            }
            McpServerConfig::Stdio { .. } => panic!("expected an Http entry, got Stdio"),
        }

        assert_eq!(oauth_credential_store_path("slack").file_name().unwrap(), "slack.json");
        assert_eq!(oauth_credential_store_path("My Server!").file_name().unwrap(), "My_Server_.json");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_non_required_path_falls_back_to_empty_config() {
        // Mirrors the default (no explicit config path) OS config path: most users won't have
        // one yet, so a missing file there should silently mean "no config", not an error.
        let path = temp_toml_path("does-not-exist");
        let config = load_config_from_path(&path, false).unwrap();

        assert!(config.provider.is_none());
        assert!(config.vault_path.is_none());
    }

    #[test]
    fn missing_required_path_errors() {
        // Mirrors an explicit `--config <path>`: the caller named this file, so a missing file
        // is a mistake worth surfacing, not silently ignored.
        let path = temp_toml_path("does-not-exist-explicit");
        let err = load_config_from_path(&path, true).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"), "error was: {err}");
    }

    #[test]
    fn malformed_config_file_errors_clearly() {
        let path = temp_toml_path("malformed");
        std::fs::write(&path, "this is not valid = = toml").unwrap();

        let err = load_config(Some(path.to_str().unwrap())).unwrap_err();
        assert!(err.to_string().contains("failed to parse config file"), "error was: {err}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_config_round_trips_through_load_config() {
        let path = temp_toml_path("save-round-trip");
        let config = FileConfig {
            provider: Some(Provider::Openai),
            model: Some("gpt-4o-mini".to_string()),
            vault_path: Some("/tmp/some-vault".to_string()),
            generated_path: Some("/tmp/some-generated".to_string()),
            enable_shell: Some(true),
            delegate_max_depth: Some(3),
            api_keys: ApiKeys {
                gemini: Some("gk".to_string()),
                openai: Some("ok".to_string()),
                tavily: Some("tk".to_string()),
                telegram_bot_token: Some("tt".to_string()),
                whisper: Some("wk".to_string()),
            },
            providers: vec![ProviderConfig {
                id: "ollama-local".to_string(),
                kind: Provider::OpenaiCompatible,
                api_key: None,
                base_url: Some("http://localhost:11434/v1".to_string()),
                model: Some("llama3.1".to_string()),
            }],
            active_provider: Some("ollama-local".to_string()),
            mcp_servers: vec![
                McpServerConfig::Stdio {
                    name: "anchor".to_string(),
                    command: "npx".to_string(),
                    args: vec!["-y".to_string(), "@anchor/mcp-server".to_string()],
                    env: std::collections::HashMap::from([("ANCHOR_API_KEY".to_string(), "secret".to_string())]),
                },
                McpServerConfig::Http {
                    name: "slack".to_string(),
                    url: "https://mcp.slack.com/mcp".to_string(),
                    headers: std::collections::HashMap::from([("Authorization".to_string(), "Bearer secret-token".to_string())]),
                    oauth: false,
                },
            ],
            agents: vec![AgentConfig {
                id: "pirate".to_string(),
                persona: "You are a pirate. Speak in pirate slang.".to_string(),
                provider_id: Some("ollama-local".to_string()),
                can_delegate_to_agents: true,
            }],
            storage_provider: Some(StorageProviderKind::DecentralizedVault),
            remote_node: Some(RemoteNodeConfig {
                server_url: "ws://100.64.0.1:7420".to_string(),
                device_id: "dev-caller".to_string(),
                device_name: "Caller Device".to_string(),
                auth_key: "shared-secret".to_string(),
                target_device_id: "dev-target".to_string(),
            }),
            git_sync: Some(GitSyncConfig { remote_url: "https://gitea.example.com/user/vault.git".to_string(), token: "pat-secret".to_string() }),
        };

        save_config(&path, &config).unwrap();
        let loaded = load_config_from_path(&path, true).unwrap();

        assert_eq!(loaded, config);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_config_creates_missing_parent_directory() {
        let parent = std::env::temp_dir().join(format!(
            "warden-bootstrap-config-test-missing-parent-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let path = parent.join("config.toml");
        assert!(!parent.exists());

        save_config(&path, &FileConfig::default()).unwrap();
        assert!(load_config_from_path(&path, true).is_ok());

        std::fs::remove_dir_all(&parent).ok();
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-bootstrap-conversations-test-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    fn sample_conversation(id: &str, updated_at: i64) -> Conversation {
        Conversation {
            id: id.to_string(),
            title: format!("Conversation {id}"),
            messages: vec![ConversationMessage {
                id: "m1".to_string(),
                role: ChatRole::User,
                content: "hello".to_string(),
                created_at: updated_at,
                usage: None,
                attachments: Vec::new(),
            }],
            created_at: updated_at,
            updated_at,
            agent_id: None,
            provider_id: None,
        }
    }

    #[test]
    fn save_conversation_round_trips_through_list_conversations() {
        let dir = temp_dir("round-trip");
        let conversation = sample_conversation("c1", 100);

        save_conversation(&dir, &conversation).unwrap();
        let loaded = list_conversations(&dir).unwrap();

        assert_eq!(loaded, vec![conversation]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_conversation_creates_missing_directory() {
        let dir = temp_dir("missing-dir");
        assert!(!dir.exists());

        save_conversation(&dir, &sample_conversation("c1", 1)).unwrap();
        assert!(dir.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_conversations_on_missing_directory_returns_empty() {
        let dir = temp_dir("does-not-exist");
        assert_eq!(list_conversations(&dir).unwrap(), Vec::new());
    }

    #[test]
    fn list_conversations_sorts_newest_updated_first() {
        let dir = temp_dir("sorting");
        save_conversation(&dir, &sample_conversation("old", 100)).unwrap();
        save_conversation(&dir, &sample_conversation("new", 200)).unwrap();

        let loaded = list_conversations(&dir).unwrap();

        assert_eq!(loaded.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_conversations_skips_a_corrupt_file_instead_of_failing() {
        let dir = temp_dir("corrupt");
        save_conversation(&dir, &sample_conversation("good", 1)).unwrap();
        std::fs::write(dir.join("corrupt.json"), "not valid json").unwrap();

        let loaded = list_conversations(&dir).unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "good");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_secret_prefers_env_over_file() {
        assert_eq!(
            resolve_secret(Some("from-env".to_string()), Some("from-file".to_string())),
            Some("from-env".to_string())
        );
        assert_eq!(resolve_secret(None, Some("from-file".to_string())), Some("from-file".to_string()));
        assert_eq!(resolve_secret(None, None), None);
    }

    #[test]
    fn resolve_flag_prefers_env_over_file() {
        assert!(resolve_flag(Some("1".to_string()), Some(false)));
        assert!(resolve_flag(Some("true".to_string()), None));
        assert!(!resolve_flag(Some("0".to_string()), Some(true)));
        assert!(!resolve_flag(Some("nonsense".to_string()), Some(true)));
        assert!(resolve_flag(None, Some(true)));
        assert!(!resolve_flag(None, Some(false)));
        assert!(!resolve_flag(None, None));
    }

    #[test]
    fn resolve_delegate_max_depth_prefers_env_over_file() {
        assert_eq!(resolve_delegate_max_depth(Some("5".to_string()), Some(1)), 5);
        assert_eq!(resolve_delegate_max_depth(Some("nonsense".to_string()), Some(1)), 1);
        assert_eq!(resolve_delegate_max_depth(None, Some(4)), 4);
        assert_eq!(resolve_delegate_max_depth(None, None), DEFAULT_DELEGATE_MAX_DEPTH);
    }

    #[test]
    fn resolve_storage_provider_prefers_env_over_file_and_defaults_to_local() {
        assert_eq!(
            resolve_storage_provider(Some("decentralized_vault".to_string()), Some(StorageProviderKind::Local)).unwrap(),
            StorageProviderKind::DecentralizedVault
        );
        assert_eq!(resolve_storage_provider(None, Some(StorageProviderKind::DecentralizedVault)).unwrap(), StorageProviderKind::DecentralizedVault);
        assert_eq!(resolve_storage_provider(None, None).unwrap(), StorageProviderKind::Local);
    }

    #[test]
    fn resolve_storage_provider_errors_on_an_unrecognized_env_value_instead_of_falling_back() {
        let err = resolve_storage_provider(Some("dropbox".to_string()), Some(StorageProviderKind::Local)).unwrap_err();
        assert!(err.to_string().contains("dropbox"));
    }

    #[test]
    fn resolve_vault_path_prefers_override_then_config_then_default() {
        let default = PathBuf::from("/default/vault");
        let config = FileConfig { vault_path: Some("/from/config".to_string()), ..Default::default() };
        let overridden = Overrides { vault_path: Some("/from/override".to_string()), ..Default::default() };

        assert_eq!(resolve_vault_path(&overridden, &config, default.clone()), PathBuf::from("/from/override"));
        assert_eq!(resolve_vault_path(&Overrides::default(), &config, default.clone()), PathBuf::from("/from/config"));
        assert_eq!(resolve_vault_path(&Overrides::default(), &FileConfig::default(), default.clone()), default);
    }

    #[test]
    fn resolve_generated_path_defaults_to_a_sibling_of_the_resolved_vault_path() {
        let vault_path = PathBuf::from("/home/user/Warden/vault");

        assert_eq!(resolve_generated_path(&FileConfig::default(), &vault_path), PathBuf::from("/home/user/Warden/generated"));
    }

    #[test]
    fn resolve_generated_path_derives_from_a_relative_vault_path_too() {
        let vault_path = PathBuf::from("vault");

        assert_eq!(resolve_generated_path(&FileConfig::default(), &vault_path), PathBuf::from("generated"));
    }

    #[test]
    fn resolve_generated_path_prefers_the_config_override() {
        let config = FileConfig { generated_path: Some("/custom/output".to_string()), ..Default::default() };

        assert_eq!(resolve_generated_path(&config, &PathBuf::from("/home/user/Warden/vault")), PathBuf::from("/custom/output"));
    }

    #[tokio::test]
    async fn build_storage_provider_supports_local_and_decentralized_vault_but_not_managed_cloud_yet() {
        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-bootstrap-storage-provider-vault-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        assert!(build_storage_provider(StorageProviderKind::Local, vault.clone(), None).await.is_ok());
        assert!(build_storage_provider(StorageProviderKind::DecentralizedVault, vault.clone(), None).await.is_ok());
        assert!(build_storage_provider(StorageProviderKind::ManagedCloud, vault, None).await.is_err());
    }

    #[tokio::test]
    async fn build_storage_provider_remote_node_requires_a_config_section() {
        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-bootstrap-storage-provider-remote-node-no-config-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        let err = expect_err(build_storage_provider(StorageProviderKind::RemoteNode, vault, None).await);
        assert!(err.contains("requires a"), "error was: {err}");
    }

    #[tokio::test]
    async fn build_storage_provider_remote_node_errors_clearly_when_the_hub_is_unreachable() {
        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-bootstrap-storage-provider-remote-node-unreachable-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        let cfg = RemoteNodeConfig {
            server_url: "ws://127.0.0.1:1".to_string(),
            device_id: "dev-caller".to_string(),
            device_name: "Caller".to_string(),
            auth_key: "test-key".to_string(),
            target_device_id: "dev-target".to_string(),
        };
        assert!(build_storage_provider(StorageProviderKind::RemoteNode, vault, Some(&cfg)).await.is_err());
    }

    #[tokio::test]
    async fn build_auth_provider_gates_only_decentralized_vault_on_truthid_pairing() {
        let manifest_path = std::env::temp_dir().join(format!(
            "warden-bootstrap-auth-provider-manifest-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));

        // Every non-decentralized kind gets `NoAuthProvider` — always "active", never errors,
        // regardless of whether `manifest_path` even exists.
        for kind in [StorageProviderKind::Local, StorageProviderKind::RemoteNode, StorageProviderKind::ManagedCloud] {
            let auth = build_auth_provider(kind, manifest_path.clone());
            assert!(auth.is_subscription_active().await.unwrap());
            assert_eq!(auth.get_user_id().await.unwrap(), None);
        }

        // `DecentralizedVault` reads the real manifest — unpaired (missing file) reports inactive
        // with no user id, matching `TruthIdAuthProvider`'s own tests.
        let auth = build_auth_provider(StorageProviderKind::DecentralizedVault, manifest_path.clone());
        assert!(!auth.is_subscription_active().await.unwrap());
        assert_eq!(auth.get_user_id().await.unwrap(), None);

        warden_sync::manifest::save_manifest(
            &manifest_path,
            &warden_sync::SyncManifest { version: 1, owner_address: Some("wallet-abc".to_string()), ..Default::default() },
        )
        .unwrap();
        let auth = build_auth_provider(StorageProviderKind::DecentralizedVault, manifest_path);
        assert!(auth.is_subscription_active().await.unwrap());
        assert_eq!(auth.get_user_id().await.unwrap(), Some("wallet-abc".to_string()));
    }

    /// `Arc<dyn ModelProvider>` isn't `Debug`, so `Result::unwrap_err` (which requires the `Ok`
    /// side to be `Debug` too) doesn't work directly on `resolve_model_provider`'s return type.
    fn expect_err<T>(result: anyhow::Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(err) => err.to_string(),
        }
    }

    fn provider_entry(id: &str, kind: Provider) -> ProviderConfig {
        ProviderConfig { id: id.to_string(), kind, api_key: Some("a-key".to_string()), base_url: None, model: Some("a-model".to_string()) }
    }

    #[test]
    fn resolve_model_provider_uses_the_registry_active_provider() {
        let config = FileConfig {
            providers: vec![provider_entry("gemini-personal", Provider::Gemini)],
            active_provider: Some("gemini-personal".to_string()),
            ..Default::default()
        };

        assert!(resolve_model_provider(&config, &Overrides::default()).is_ok());
    }

    #[test]
    fn resolve_model_provider_override_provider_id_wins_over_active_provider() {
        let config = FileConfig {
            providers: vec![provider_entry("a", Provider::Gemini), provider_entry("b", Provider::Openai)],
            active_provider: Some("a".to_string()),
            ..Default::default()
        };
        let overrides = Overrides { provider_id: Some("b".to_string()), ..Default::default() };

        assert!(resolve_model_provider(&config, &overrides).is_ok());
    }

    #[test]
    fn resolve_model_provider_errors_when_active_provider_is_unset() {
        let config = FileConfig { providers: vec![provider_entry("a", Provider::Gemini)], ..Default::default() };

        let err = expect_err(resolve_model_provider(&config, &Overrides::default()));
        assert!(err.contains("no `active_provider` is set"), "error was: {err}");
    }

    #[test]
    fn resolve_model_provider_errors_when_active_provider_id_is_unknown() {
        let config = FileConfig {
            providers: vec![provider_entry("a", Provider::Gemini)],
            active_provider: Some("does-not-exist".to_string()),
            ..Default::default()
        };

        let err = expect_err(resolve_model_provider(&config, &Overrides::default()));
        assert!(err.contains("does-not-exist"), "error was: {err}");
    }

    #[test]
    fn resolve_model_provider_openai_compatible_requires_a_base_url() {
        let mut entry = provider_entry("ollama", Provider::OpenaiCompatible);
        entry.base_url = None;
        let config = FileConfig { providers: vec![entry], active_provider: Some("ollama".to_string()), ..Default::default() };

        let err = expect_err(resolve_model_provider(&config, &Overrides::default()));
        assert!(err.contains("base_url"), "error was: {err}");
    }

    #[test]
    fn resolve_model_provider_openai_compatible_works_without_an_api_key() {
        let mut entry = provider_entry("ollama", Provider::OpenaiCompatible);
        entry.api_key = None;
        entry.base_url = Some("http://localhost:11434/v1".to_string());
        let config = FileConfig { providers: vec![entry], active_provider: Some("ollama".to_string()), ..Default::default() };

        assert!(resolve_model_provider(&config, &Overrides::default()).is_ok());
    }

    #[test]
    fn resolve_model_provider_falls_back_to_the_legacy_single_provider_fields_when_the_registry_is_empty() {
        let config = FileConfig {
            provider: Some(Provider::Openai),
            api_keys: ApiKeys { openai: Some("legacy-key".to_string()), ..Default::default() },
            ..Default::default()
        };

        assert!(resolve_model_provider(&config, &Overrides::default()).is_ok());
    }

    #[test]
    fn resolve_model_provider_legacy_fallback_errors_clearly_with_no_key_anywhere() {
        // Anthropic has no legacy env-var/config-file slot to fall back to, so this is
        // deterministic regardless of the ambient environment (unlike Gemini/OpenAI, whose
        // legacy fallback also consults GEMINI_API_KEY/OPENAI_API_KEY from the real process env).
        let overrides = Overrides { provider: Some(Provider::Anthropic), ..Default::default() };

        let err = expect_err(resolve_model_provider(&FileConfig::default(), &overrides));
        assert!(err.contains("anthropic"), "error was: {err}");
    }

    #[test]
    fn default_model_for_has_no_universal_default_for_openai_compatible() {
        assert_eq!(default_model_for(Provider::OpenaiCompatible), None);
        assert!(default_model_for(Provider::Gemini).is_some());
        assert!(default_model_for(Provider::Openai).is_some());
        assert!(default_model_for(Provider::Anthropic).is_some());
    }

    fn provider_config(id: &str) -> ProviderConfig {
        ProviderConfig { id: id.to_string(), kind: Provider::Gemini, api_key: None, base_url: None, model: None }
    }

    fn agent_config(id: &str, provider_id: Option<&str>) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            persona: String::new(),
            provider_id: provider_id.map(str::to_string),
            can_delegate_to_agents: false,
        }
    }

    #[test]
    fn rename_provider_cascade_updates_active_provider_and_referencing_agents() {
        let mut config = FileConfig {
            providers: vec![provider_config("old-id")],
            active_provider: Some("old-id".to_string()),
            agents: vec![agent_config("a1", Some("old-id")), agent_config("a2", Some("someone-else"))],
            ..Default::default()
        };

        rename_provider_cascade(&mut config, "old-id", "new-id");

        assert_eq!(config.active_provider.as_deref(), Some("new-id"));
        assert_eq!(config.agents[0].provider_id.as_deref(), Some("new-id"));
        assert_eq!(config.agents[1].provider_id.as_deref(), Some("someone-else"));
    }

    #[test]
    fn rename_provider_cascade_leaves_unrelated_references_untouched() {
        let mut config = FileConfig { active_provider: Some("other".to_string()), agents: vec![agent_config("a1", None)], ..Default::default() };

        rename_provider_cascade(&mut config, "old-id", "new-id");

        assert_eq!(config.active_provider.as_deref(), Some("other"));
        assert_eq!(config.agents[0].provider_id, None);
    }

    #[test]
    fn remove_provider_references_clears_active_provider_and_referencing_agents() {
        let mut config = FileConfig {
            active_provider: Some("gone".to_string()),
            agents: vec![agent_config("a1", Some("gone")), agent_config("a2", Some("stays"))],
            ..Default::default()
        };

        remove_provider_references(&mut config, "gone");

        assert_eq!(config.active_provider, None);
        assert_eq!(config.agents[0].provider_id, None);
        assert_eq!(config.agents[1].provider_id.as_deref(), Some("stays"));
    }

    #[test]
    fn seed_default_vault_files_writes_every_fixed_file_with_nonempty_content() {
        let vault = Vault::new(temp_dir("seed-fresh"));

        seed_default_vault_files(&vault);

        for name in FIXED_VAULT_FILES {
            let content = vault.read(name).unwrap();
            assert!(!content.trim().is_empty(), "{name} should have been seeded with a template");
        }

        std::fs::remove_dir_all(vault.root()).ok();
    }

    #[test]
    fn seed_default_vault_files_never_overwrites_an_existing_file() {
        let vault = Vault::new(temp_dir("seed-existing"));
        vault.write("_profile.md", "already customized by the user").unwrap();

        seed_default_vault_files(&vault);

        assert_eq!(vault.read("_profile.md").unwrap(), "already customized by the user");

        std::fs::remove_dir_all(vault.root()).ok();
    }
}
