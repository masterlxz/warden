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
use warden_core::tool::delegate_to_agent::{AgentResolver, AgentsRevision, DelegateToAgentTool, NamedSubAgent};
use warden_core::tool::job_tools::JobsTool;
use warden_core::tool::document::GenerateDocumentTool;
use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
use warden_core::tool::mcp::McpToolProvider;
use warden_core::skill::SkillStore;
use warden_core::tool::shell::ShellTool;
use warden_core::tool::skill_tools::{ManageSkillTool, ReadSkillFileTool, UseSkillTool};
use warden_core::tool::spend_tool::BudgetTool;
use warden_core::tool::ssh::{ssh_tools, AuditLog, SshHost};
use warden_core::tool::{Tool, ToolProvider};

mod config_file;
pub mod manage_agents;
pub mod settings;
pub mod skill_gen;
pub mod spend;
pub mod usage;
pub use config_file::render_config;
pub use manage_agents::ManageAgentsTool;
pub use spend::{default_limit_configs, default_spend_ledger_path, env_switches_limits_off, LimitConfig, LimitScope};
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
    /// Opt-in (P46) for the `manage_agents` tool — only an agent with this set gets it, and it lets
    /// that agent list, create and edit *other* agents (every change waits for the user's yes). It
    /// can never grant itself, or any agent it creates, this flag or `can_delegate_to_agents`: those
    /// are switched on by a human in the Settings screen / `/agents` only.
    #[serde(default)]
    pub can_manage_agents: bool,
    /// Tool isolation (P46): the only tools this agent may use, by name. `None` (the default, and
    /// what every config.toml written before this field means) keeps every tool. `delegate_to_agent`
    /// and `manage_agents` never belong here — they follow the two `can_*` flags above. Applied in
    /// code (`Orchestrator::with_allowed_tools`), so it holds however the agent is reached: as the
    /// conversation's agent or as a `delegate_to_agent` target.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
}

/// What an agent created by another agent may use unless the creator asks for more (and the user
/// approves it): read-only access and document output — no `write_file`, `shell`, `ssh_*`,
/// `manage_skill`, `delegate_task`, and no MCP tool. `budget` is read-only too, and an agent that can
/// see how close it is to its spending limit is one that can decide to stop (P4).
pub const SAFE_AGENT_TOOLS: [&str; 6] = ["read_file", "use_skill", "read_skill_file", "usage_stats", "budget", "generate_document"];

/// One SSH server the AI may run commands on through the `ssh_exec` tool (P47). Edited from the
/// desktop Settings screen or the CLI's `/ssh`. Only the *path* to a private key lives here, never
/// the key itself, and there is no passphrase field: a protected key has to be loaded in the
/// user's ssh-agent.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SshHostConfig {
    /// User-chosen, unique among `ssh_hosts` — the only handle the model uses to pick a server.
    pub id: String,
    pub host: String,
    pub user: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Path to a private key (passed as `ssh -i`). `None` leaves it to ssh-agent and `~/.ssh/config`.
    #[serde(default)]
    pub identity_file: Option<String>,
    /// Master switch. `false` (also the default for a hand-written entry that omits it) keeps the
    /// host registered but invisible to the model.
    #[serde(default)]
    pub enabled: bool,
    /// Agents allowed to use this host. Empty means every agent and every channel without an agent
    /// (Telegram, WhatsApp, mobile, the MCP server) — the same trust as the `shell` tool.
    #[serde(default)]
    pub agents: Vec<String>,
    /// Ask a human before every command or file transfer on this host. Only the desktop and the
    /// interactive CLI can ask; any other channel (Telegram, WhatsApp, mobile, the MCP server,
    /// sub-agents) refuses instead of running it unattended.
    #[serde(default)]
    pub require_approval: bool,
}

fn default_ssh_port() -> u16 {
    22
}

impl SshHostConfig {
    /// The core-side shape, which has no `enabled` flag (disabled hosts never reach the tool).
    pub fn to_host(&self) -> SshHost {
        SshHost {
            id: self.id.clone(),
            host: self.host.clone(),
            user: self.user.clone(),
            port: self.port,
            identity_file: self.identity_file.clone(),
            agents: self.agents.clone(),
            require_approval: self.require_approval,
        }
    }
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

/// Config for the desktop app embedding its own `warden-server` hub (Fase 9.1 follow-up, "virar o
/// hub desta rede") instead of that always being a separate process. `enabled: true` makes
/// `desktop/src-tauri`'s `run()` start it automatically on every launch — this is meant to behave
/// like an always-on home service, not a per-session toggle. Every `warden-server serve` flag has
/// a field here (Sessão 103), so the desktop can do whatever the terminal can. `auth_key` is
/// generated by default (see `generate_auth_key`) and must pass `is_strong_auth_key`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedServerConfig {
    pub enabled: bool,
    pub port: u16,
    /// Address to listen on — `serve --listen`'s host part. `None` is `0.0.0.0` (every local
    /// interface); `127.0.0.1` keeps the hub reachable only from this machine.
    #[serde(default)]
    pub listen_host: Option<String>,
    pub auth_key: String,
    /// Shown in `HelloAck`/`DiscoverAck` so other devices can recognize which machine this is.
    /// `None` falls back to the same hostname/literal chain `warden-server`'s own `--server-name`
    /// resolution already uses (`resolve_server_name` in `crates/warden-server/src/main.rs`) —
    /// this struct doesn't duplicate that logic, the desktop command that starts the server does.
    pub server_name: Option<String>,
    /// Serve only `wss://`, with this machine's Tailscale certificate (P36) — same as
    /// `warden-server serve --tailscale-cert`. Absent in older files = off.
    #[serde(default)]
    pub tailscale_cert: bool,
    /// Serve only `wss://` with this PEM certificate chain and key instead — `serve --tls-cert`/
    /// `--tls-key`. Both or neither, and not together with `tailscale_cert`.
    #[serde(default)]
    pub tls_cert: Option<String>,
    #[serde(default)]
    pub tls_key: Option<String>,
    /// The name `tls_cert` is valid for, advertised to discovery — `serve --tls-host`.
    #[serde(default)]
    pub tls_host: Option<String>,
    /// Serve the web interface (P78) on the same port — `false` is `serve --no-web-ui`.
    #[serde(default = "default_true")]
    pub web_ui: bool,
}

fn default_true() -> bool {
    true
}

impl EmbeddedServerConfig {
    /// Switched off, on every interface, plain `ws://`, with the web interface — what a fresh
    /// `serve --listen 0.0.0.0:<port>` does.
    pub fn new(port: u16, auth_key: impl Into<String>) -> Self {
        Self {
            enabled: false,
            port,
            listen_host: None,
            auth_key: auth_key.into(),
            server_name: None,
            tailscale_cert: false,
            tls_cert: None,
            tls_key: None,
            tls_host: None,
            web_ui: true,
        }
    }
}

/// 32 random bytes from the OS CSPRNG, hex-encoded (64 hex chars) — used for `EmbeddedServerConfig
/// ::auth_key` so the desktop's embedded hub never starts with a weak or empty credential. Plain
/// `rand`/`OsRng` rather than reusing any of `warden-truthid`'s crypto primitives: those are all
/// keyed to a specific protocol (ECIES, pairing codes), not a generic "give me a random secret"
/// helper, and pulling one in for that would be a stranger dependency than just adding `rand`.
pub fn generate_auth_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Shortest pairing key a hub will start with (P83) — half of what `generate_auth_key` produces.
/// The pairing key also guards saving settings from the web (P78), where a wrong guess only costs
/// a 1 s wait, so a short hand-typed key would be the weak point.
pub const MIN_AUTH_KEY_LEN: usize = 32;

/// Whether `key` is long enough to be a hub's pairing key (see `MIN_AUTH_KEY_LEN`). Callers word
/// their own refusal — the CLI in English, the desktop in Portuguese.
pub fn is_strong_auth_key(key: &str) -> bool {
    key.trim().chars().count() >= MIN_AUTH_KEY_LEN
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
    /// How many model calls the sub-agents (`delegate_task`, `delegate_to_agent`, and what those
    /// delegate to) may make between them in one turn (P46/P60/P18). `None` keeps
    /// `DEFAULT_MAX_DELEGATED_CALLS`; `WARDEN_MAX_DELEGATED_CALLS` wins over this if set; `0` turns
    /// the limit off. No UI — config.toml/env only, same posture as `delegate_max_depth`.
    pub max_delegated_calls: Option<u32>,
    /// How many background jobs (`background: true` on a delegation call, P46) one turn may run at
    /// the same time; the rest wait in the queue. `None` keeps `DEFAULT_MAX_PARALLEL_JOBS`;
    /// `WARDEN_MAX_PARALLEL_JOBS` wins over this if set; `0` means one at a time (jobs stay on, they
    /// just never overlap). Every job still spends from `max_delegated_calls`. No UI — config.toml/env
    /// only, same posture as `delegate_max_depth`.
    pub max_parallel_jobs: Option<u32>,
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
    /// The desktop app embedding its own `warden-server` hub (Fase 9.1 follow-up) — `None` means
    /// never configured (equivalent to `enabled: false`, but distinct so the Settings UI can tell
    /// "never set up" from "set up, currently off"). See `EmbeddedServerConfig`'s own doc comment.
    pub embedded_server: Option<EmbeddedServerConfig>,
    /// SSH servers the AI can run commands on (P47) — empty by default, so no `ssh_exec` tool
    /// exists until at least one *enabled* host is registered.
    #[serde(default)]
    pub ssh_hosts: Vec<SshHostConfig>,
    /// Spending limits (P4, TOML `[[limits]]`): how many tokens and/or dollars a sliding window of
    /// time may cost, per scope (everything, an agent, a channel, one user of a channel). `None`
    /// (no `[[limits]]` in the file) keeps the built-in safety net (`spend::default_limits`);
    /// `limits = []` turns every limit off. `WARDEN_SPEND_LIMITS=off` also does, over the file.
    /// See the `spend` module for the entry format. No Settings-screen UI yet.
    pub limits: Option<Vec<LimitConfig>>,
    /// What each model charges per million tokens (TOML `[[prices]]`), so a limit can be in dollars.
    /// Nothing is built in — a model listed nowhere counts against token limits only.
    #[serde(default)]
    pub prices: Vec<warden_core::spend::Price>,
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
/// settings UI). An existing file keeps its comments and layout (P82, see `render_config`).
pub fn save_config(path: &Path, config: &FileConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory at {}", parent.display()))?;
    }
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).with_context(|| format!("failed to read config file at {}", path.display())),
    };
    let contents = render_config(existing.as_deref(), config)?;
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

/// What removing an agent did to one SSH host that named it (P46, `manage_agents` delete).
#[derive(Debug, Clone, PartialEq)]
pub struct SshHostEffect {
    pub host_id: String,
    /// The host was restricted to this agent alone, so it is now switched off — an empty `agents`
    /// list means "every agent and channel", and pruning must never widen access.
    pub switched_off: bool,
}

/// Removes `agent_id` from `agents` and from every SSH host that lists it, returning what happened
/// to those hosts. A host left with no agent is switched off instead of falling through to "every
/// agent" (the desktop's Settings screen does the same when an agent is deleted there), and a
/// dangling reference is never left behind: the desktop's `save_settings` rejects one.
pub fn remove_agent_from(agents: &mut Vec<AgentConfig>, ssh_hosts: &mut [SshHostConfig], agent_id: &str) -> Vec<SshHostEffect> {
    agents.retain(|a| a.id != agent_id);
    let mut effects = Vec::new();
    for host in ssh_hosts.iter_mut().filter(|h| h.agents.iter().any(|a| a == agent_id)) {
        host.agents.retain(|a| a != agent_id);
        let switched_off = host.agents.is_empty();
        if switched_off {
            host.enabled = false;
        }
        effects.push(SshHostEffect { host_id: host.id.clone(), switched_off });
    }
    effects
}

/// `remove_agent_from` over a whole `FileConfig` — for callers that hold one (the CLI's `/agents remove`).
pub fn remove_agent_references(config: &mut FileConfig, agent_id: &str) -> Vec<SshHostEffect> {
    remove_agent_from(&mut config.agents, &mut config.ssh_hosts, agent_id)
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
    /// Media attached to this message — user-attached images on the user turn (P28), or media
    /// extracted from an MCP tool's `CallToolResult` on the assistant turn (P64 frente 2).
    /// `#[serde(default)]` so conversations saved before this field existed still load.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Paths of files actually written to disk on the assistant turn (P64) — `generate_document`
    /// or oversized MCP media spilled to disk (`Orchestrator::MessageOutcome::generated_files`).
    /// `#[serde(default)]` so conversations saved before this field existed still load.
    #[serde(default)]
    pub generated_files: Vec<String>,
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

/// Where a hub keeps the TLS certificate it fetches with `tailscale cert` (P36) — shared by
/// `warden-server serve --tailscale-cert` and the desktop's embedded hub.
pub fn default_tls_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("tls"))
}

/// The *client* side of P36's device tokens — what this machine was issued by each hub it pairs
/// with as a Rust client (`warden-node`, a `[remote_node]` storage provider). Separate from
/// `devices.json`, which is the hub's own registry of the devices pairing with *it*.
pub fn default_client_device_tokens_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("device_tokens.json"))
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

/// Where every `ssh_exec`/`ssh_upload`/`ssh_download` call is recorded (P47) — JSONL, one line per
/// call, same `dirs::config_dir()` base as the other Warden files. `None` when there's no config
/// directory (the tools then simply run without an audit log).
pub fn default_ssh_audit_log_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("ssh_audit.jsonl"))
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
/// by every channel that threads conversations this way — Telegram, WhatsApp and the hub.
/// `attachments` go with the user's turn (images or PDFs, P78) and are saved on it, so later turns
/// resend them to the model like the desktop does.
/// `warden-cli`'s REPL and the desktop app don't use this: the CLI has no persistence at all, and
/// desktop's frontend already does its own read/append/save around the IPC boundary.
pub async fn handle_turn(
    orchestrator: &Orchestrator,
    conversations_dir: &Path,
    conversation_id: &str,
    title_seed: &str,
    user_input: &str,
    attachments: Vec<Attachment>,
) -> anyhow::Result<MessageOutcome> {
    let existing = load_conversation(conversations_dir, conversation_id)?;
    let existed = existing.is_some();
    let history: Vec<Message> = existing.iter().flat_map(|c| &c.messages).map(to_message).collect();
    let outcome = orchestrator.handle_message_with_attachments(&history, user_input, attachments.clone()).await?;

    // P78: the model call can take a minute, and in the meantime a hub client may have renamed or
    // deleted this conversation (or finished another turn in it) — so the file is read again here,
    // under the same lock `rename_conversation`/`delete_conversation` take, instead of saving the
    // copy loaded above over whatever changed.
    let _guard = CONVERSATION_WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut conversation = match load_conversation(conversations_dir, conversation_id)? {
        Some(conversation) => conversation,
        // Deleted while the model answered — the answer still goes back, but the conversation
        // stays deleted instead of coming back with this one turn in it.
        None if existed => return Ok(outcome),
        None => {
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
        }
    };

    conversation.messages.push(ConversationMessage {
        id: message_id(),
        role: ChatRole::User,
        content: user_input.to_string(),
        created_at: now_millis(),
        usage: None,
        attachments,
        generated_files: Vec::new(),
    });
    conversation.messages.push(ConversationMessage {
        id: message_id(),
        role: ChatRole::Assistant,
        content: outcome.content.clone(),
        created_at: now_millis(),
        usage: outcome.usage,
        attachments: outcome.attachments.clone(),
        generated_files: outcome.generated_files.clone(),
    });
    conversation.updated_at = now_millis();

    save_conversation(conversations_dir, &conversation)?;
    Ok(outcome)
}

/// Serializes the read-modify-write of a conversation file between `handle_turn`'s save and
/// `rename_conversation`/`delete_conversation` (P78), within this process — the hub is the only
/// writer of its conversations directory. Held only around file I/O, never the model call.
static CONVERSATION_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The longest title `rename_conversation` keeps — a sidebar label, not a place for prose.
pub const MAX_CONVERSATION_TITLE_CHARS: usize = 120;

/// Renames a saved conversation (P78). The title is trimmed and cut to
/// `MAX_CONVERSATION_TITLE_CHARS`; an empty one is an error. `false` when there's no such
/// conversation. `updated_at` is left alone: renaming isn't activity, so it doesn't reorder the list.
pub fn rename_conversation(dir: &Path, id: &str, title: &str) -> anyhow::Result<bool> {
    let title = title.trim();
    anyhow::ensure!(!title.is_empty(), "a conversation title can't be empty");
    let _guard = CONVERSATION_WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(mut conversation) = load_conversation(dir, id)? else {
        return Ok(false);
    };
    conversation.title = title.chars().take(MAX_CONVERSATION_TITLE_CHARS).collect();
    save_conversation(dir, &conversation)?;
    Ok(true)
}

/// Deletes a saved conversation's file (P78). `false` when there was none.
pub fn delete_conversation(dir: &Path, id: &str) -> anyhow::Result<bool> {
    let _guard = CONVERSATION_WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = dir.join(format!("{id}.json"));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("failed to delete conversation file at {}", path.display())),
    }
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

/// Same precedence and same leniency as `resolve_delegate_max_depth`, for `max_delegated_calls`.
/// `0` is a real answer ("no limit"), not "unset".
pub fn resolve_max_delegated_calls(from_env: Option<String>, from_file: Option<u32>) -> u32 {
    from_env.and_then(|v| v.trim().parse().ok()).or(from_file).unwrap_or(DEFAULT_MAX_DELEGATED_CALLS)
}

/// Same precedence and leniency again, for `max_parallel_jobs`. Unlike the two limits above, `0`
/// does not switch the feature off — it means "one at a time" (the job queue clamps to 1).
pub fn resolve_max_parallel_jobs(from_env: Option<String>, from_file: Option<u32>) -> u32 {
    from_env.and_then(|v| v.trim().parse().ok()).or(from_file).unwrap_or(DEFAULT_MAX_PARALLEL_JOBS)
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
///
/// Generic over `ToolProvider` rather than tied to `McpToolProvider` specifically (both real call
/// sites still pass one, inferred) — the only thing used is `tools()`, the trait method, and going
/// generic is what lets this be tested with a fake provider instead of a real MCP process (P46).
///
/// Each tool's name is deduped against everything already in `base_tools` (built-ins registered
/// earlier, Tavily, and any `[[mcp_servers]]` entry already processed this call) via
/// `warden_core::tool::dedupe_tool_name` (shared with `warden-server`'s own collision point, P42);
/// a rename is logged so whoever writes `allowed_tools` knows the name to use.
async fn register_mcp_tools<P: ToolProvider>(base_tools: &mut Vec<Arc<dyn Tool>>, name: &str, connect_result: anyhow::Result<P>) {
    match connect_result {
        Ok(provider) => match provider.tools().await {
            Ok(tools) => {
                for tool in tools {
                    let original = tool.spec().name;
                    let existing: Vec<String> = base_tools.iter().map(|t| t.spec().name).collect();
                    let resolved = warden_core::tool::dedupe_tool_name(&existing, name, &original);
                    if resolved == original {
                        base_tools.push(tool);
                    } else {
                        eprintln!(
                            "note: MCP server '{name}' tool '{original}' collides with an already-registered tool — \
                             renamed to '{resolved}' (use this name in allowed_tools)\n"
                        );
                        base_tools.push(warden_core::tool::rename_tool(tool, resolved));
                    }
                }
            }
            Err(err) => eprintln!("note: MCP server '{name}' connected but failed to list tools: {err:#}\n"),
        },
        Err(err) => eprintln!("note: MCP server '{name}' unavailable, skipping: {err:#}\n"),
    }
}

/// Per-channel overrides (CLI flags today; a desktop settings UI later — see PHASE.md 6.5).
#[derive(Default, Clone)]
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
        StorageProviderKind::DecentralizedVault => {
            // Same default-path helpers desktop's own `AppState.sync` already uses to build a
            // `SyncEngine` (`desktop/src-tauri/src/lib.rs`'s `sync_secrets_path`/
            // `sync_manifest_path`) — `warden-sync` can't call `default_config_path` itself
            // (`warden-bootstrap` depends on `warden-sync`, not the other way around), so this is
            // the one place that can assemble the engine `DecentralizedVaultProvider` needs for
            // its `_interactive` methods (P61 follow-up) to actually reach Arweave.
            let config_path = default_config_path().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?;
            let secrets_path = warden_sync::paths::default_sync_secrets_path().unwrap_or_else(|| PathBuf::from("sync_secrets.json"));
            let manifest_path = warden_sync::paths::default_sync_manifest_path().unwrap_or_else(|| PathBuf::from("sync_manifest.json"));
            let sync = warden_sync::SyncEngine::new(vault.root().to_path_buf(), config_path, secrets_path, manifest_path);
            Arc::new(warden_sync::DecentralizedVaultProvider::new(vault, sync))
        }
        StorageProviderKind::RemoteNode => {
            let cfg = remote_node
                .ok_or_else(|| anyhow::anyhow!("storage_provider 'remote_node' requires a [remote_node] config section"))?;
            let tokens_path = default_client_device_tokens_path().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?;
            Arc::new(
                warden_server_protocol::RemoteNodeProvider::connect(
                    &cfg.server_url,
                    &cfg.device_id,
                    &cfg.device_name,
                    &cfg.auth_key,
                    cfg.target_device_id.clone(),
                    &warden_server_protocol::DeviceTokenStore::new(tokens_path),
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
/// everyone else, just its own persona/model/tool list (`allowed_tools`, P46) layered on top — so
/// pass the orchestrator *before* narrowing it to the chief's own `allowed_tools`, or every target
/// would inherit the chief's limits instead of its own.
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
fn delegate_targets(config: &FileConfig, orchestrator: &Orchestrator) -> Vec<NamedSubAgent> {
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
        // The delegated agent sees its own skills (P72 c) and only its own tools (P46), not the
        // chief's — which is why `orchestrator` must reach this function unrestricted.
        let target_orchestrator =
            target_orchestrator.with_agent(Some(agent.id.clone())).with_allowed_tools(agent.allowed_tools.as_deref());
        let persona = (!agent.persona.trim().is_empty()).then(|| agent.persona.clone());
        targets.push(NamedSubAgent {
            id: agent.id.clone(),
            description: agent.persona.clone(),
            orchestrator: target_orchestrator,
            persona,
        });
    }
    targets
}

pub fn build_delegate_to_agent_tool(config: &FileConfig, orchestrator: &Orchestrator) -> Option<Arc<dyn Tool>> {
    let targets = delegate_targets(config, orchestrator);
    (!targets.is_empty()).then(|| Arc::new(DelegateToAgentTool::new(targets)) as Arc<dyn Tool>)
}

/// Same tool as `build_delegate_to_agent_tool`, but its target list follows the agents on disk
/// during the turn: when `revision` moves (a `ManageAgentsTool` given the same `AgentsRevision`
/// created, edited or deleted an agent), the targets are rebuilt from `config_path` — so an agent
/// the chief just created can be delegated to in that same turn instead of from the next one. The
/// rebuilt targets clone the same `orchestrator` the first ones did, so they carry exactly the
/// same tools and delegation depth. An unreadable config keeps the previous list.
pub fn build_live_delegate_to_agent_tool(
    config_path: &Path,
    config: &FileConfig,
    orchestrator: &Orchestrator,
    revision: AgentsRevision,
) -> Option<Arc<dyn Tool>> {
    let targets = delegate_targets(config, orchestrator);
    if targets.is_empty() {
        return None;
    }
    let path = config_path.to_path_buf();
    let base = orchestrator.clone();
    let resolver: AgentResolver = Arc::new(move || {
        let config = load_config_from_path(&path, false).ok()?;
        Some(delegate_targets(&config, &base))
    });
    Some(Arc::new(DelegateToAgentTool::live(targets, revision, resolver)))
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

    // Read here, before `config` is picked apart below (the Tavily key is moved out of it).
    let spend_guard = spend::build_spend_guard(&config);

    let mut base_tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ReadFileTool::new(vault.clone())),
        Arc::new(WriteFileTool::new(vault.clone())),
        Arc::new(GenerateDocumentTool::new(generated_path.clone())),
        Arc::new(UsageStatsTool::new(default_conversations_dir())),
        Arc::new(UseSkillTool::new(SkillStore::new(vault.clone()))),
        Arc::new(ReadSkillFileTool::new(SkillStore::new(vault.clone()))),
        Arc::new(ManageSkillTool::new(SkillStore::new(vault.clone()))),
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

    base_tools.extend(build_ssh_tools(&config.ssh_hosts, vault.root().clone(), default_ssh_audit_log_path()));

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
    let max_delegated_calls =
        resolve_max_delegated_calls(std::env::var("WARDEN_MAX_DELEGATED_CALLS").ok(), config.max_delegated_calls);
    let max_parallel_jobs =
        resolve_max_parallel_jobs(std::env::var("WARDEN_MAX_PARALLEL_JOBS").ok(), config.max_parallel_jobs);
    // Reads background jobs' results (P46). Registered unbound: the orchestrator binds it to each turn's
    // job board, and until then it is hidden from the model — sub-agents, which inherit it, never see it.
    base_tools.push(Arc::new(JobsTool::new()));
    // Shows the spending meter (P4). Same story as `jobs`: bound to each turn by the orchestrator, and
    // hidden from the model whenever the turn has no limits.
    base_tools.push(Arc::new(BudgetTool::new()));
    let mut orchestrator = build_delegating_orchestrator(model_provider, vault, &base_tools, delegate_max_depth, generated_path)
        .with_delegation_limit(max_delegated_calls)
        .with_parallel_jobs(max_parallel_jobs as usize);
    if let Some(guard) = spend_guard {
        orchestrator = orchestrator.with_spend_guard(guard);
    }

    Ok(orchestrator)
}

/// The `ssh_exec`/`ssh_upload`/`ssh_download` tools for the enabled hosts in `entries`, or none
/// when there are no such hosts — same "the tool doesn't exist until you turn it on" posture as
/// `shell`, without a global flag since each host already has its own switch. A host that fails
/// validation (e.g. a hand-edited `host = "-oProxyCommand=..."`) is skipped with a note rather than
/// aborting startup. Relative local paths in the transfer tools resolve against `base_dir`.
fn build_ssh_tools(entries: &[SshHostConfig], base_dir: PathBuf, audit_log: Option<PathBuf>) -> Vec<Arc<dyn Tool>> {
    let mut hosts = Vec::new();
    for entry in entries.iter().filter(|h| h.enabled) {
        let host = entry.to_host();
        match host.validate() {
            Ok(()) => hosts.push(host),
            Err(err) => eprintln!("note: ssh host skipped — {err:#}\n"),
        }
    }
    if hosts.is_empty() {
        return Vec::new();
    }
    ssh_tools(hosts, base_dir, audit_log.map(|path| Arc::new(AuditLog::new(path))))
}

/// Default for how many levels deep a sub-agent spawned via `DelegateTool` can itself delegate
/// further (P46 — "sub-agentes autônomos", core recursion piece), used when `delegate_max_depth`
/// isn't set in config.toml/env (see `resolve_delegate_max_depth`). Depth alone doesn't bound the
/// cost: a chain's worst case is roughly `MAX_TOOL_ITERATIONS ^ depth` model calls if every
/// iteration at every level delegates, so a small default keeps that sane out of the box, and the
/// per-turn `max_delegated_calls` (`DEFAULT_MAX_DELEGATED_CALLS`) caps what the whole tree may
/// actually spend — see `PENDING.md` P60.
const DEFAULT_DELEGATE_MAX_DEPTH: u32 = 2;

/// Default for `max_delegated_calls`: the model calls all the sub-agents of one turn may make in
/// total. Room for a few real sub-tasks (about three of eight calls each) while staying well under
/// the `MAX_TOOL_ITERATIONS ^ depth` worst case a runaway chain could otherwise reach (P60). The
/// turn's own agent isn't counted, so it can always answer once the limit is hit.
const DEFAULT_MAX_DELEGATED_CALLS: u32 = 30;

/// Default for `max_parallel_jobs`: how many background sub-agent jobs of one turn run at once.
/// Small on purpose — each is a full model conversation, and providers rate-limit concurrent calls.
const DEFAULT_MAX_PARALLEL_JOBS: u32 = 3;

/// Builds an `Orchestrator` with `base_tools` registered, plus — while `depth > 0` — a
/// `DelegateTool` wrapping another orchestrator built the same way one level shallower. The
/// terminal orchestrator (`depth == 0`) never gets a `DelegateTool`, so it never advertises
/// `delegate_task` in its tool specs — that's the actual stopping criterion (structural, not a
/// runtime check), see the doc comment on `DelegateTool` itself.
///
/// `media_root` (P64/P66 — where oversized MCP media gets spilled to disk instead of dumped as
/// text) is applied at every depth, not just the top level: a sub-agent's own tool calls can
/// return oversized media too.
fn build_delegating_orchestrator(
    model: Arc<dyn ModelProvider>,
    vault: Arc<Vault>,
    base_tools: &[Arc<dyn Tool>],
    depth: u32,
    media_root: PathBuf,
) -> Orchestrator {
    let mut orchestrator = Orchestrator::new(model.clone(), vault.clone()).with_media_root(media_root.clone());
    for tool in base_tools {
        orchestrator.register_tool(tool.clone());
    }
    if depth > 0 {
        let sub = build_delegating_orchestrator(model, vault, base_tools, depth - 1, media_root);
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
            max_delegated_calls: Some(12),
            max_parallel_jobs: Some(2),
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
                can_manage_agents: true,
                allowed_tools: Some(vec!["read_file".to_string(), "use_skill".to_string()]),
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
            embedded_server: Some(EmbeddedServerConfig {
                enabled: true,
                port: 7420,
                listen_host: Some("127.0.0.1".to_string()),
                auth_key: "embedded-secret".to_string(),
                server_name: Some("Fabio's Desktop".to_string()),
                tailscale_cert: true,
                tls_cert: Some("/etc/hub/cert.pem".to_string()),
                tls_key: Some("/etc/hub/key.pem".to_string()),
                tls_host: Some("hub.example.com".to_string()),
                web_ui: false,
            }),
            ssh_hosts: vec![SshHostConfig {
                id: "vps".to_string(),
                host: "203.0.113.7".to_string(),
                user: "deploy".to_string(),
                port: 2222,
                identity_file: Some("/home/me/.ssh/id_ed25519".to_string()),
                enabled: true,
                agents: vec!["ops".to_string()],
                require_approval: true,
            }],
            limits: Some(vec![LimitConfig {
                id: "ana-hour".to_string(),
                scope: LimitScope::User,
                target: Some("telegram:42".to_string()),
                window_hours: 1,
                max_tokens: Some(100_000),
                max_cost_usd: Some(0.5),
                warn_at: Some(0.7),
                extend_step: None,
            }]),
            prices: vec![warden_core::spend::Price { model: "gpt-4o-mini".to_string(), input_per_mtok: 0.15, output_per_mtok: 0.6 }],
        };

        save_config(&path, &config).unwrap();
        let loaded = load_config_from_path(&path, true).unwrap();

        assert_eq!(loaded, config);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn generate_auth_key_produces_64_hex_chars_and_never_repeats() {
        let a = generate_auth_key();
        let b = generate_auth_key();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(is_strong_auth_key(&a));
    }

    #[test]
    fn short_or_padded_auth_keys_are_not_strong() {
        assert!(!is_strong_auth_key(""));
        assert!(!is_strong_auth_key("secret"));
        assert!(!is_strong_auth_key(&format!("  {}  ", "a".repeat(MIN_AUTH_KEY_LEN - 1))));
        assert!(is_strong_auth_key(&"a".repeat(MIN_AUTH_KEY_LEN)));
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
                generated_files: Vec::new(),
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
    fn rename_conversation_trims_keeps_updated_at_and_reports_a_missing_one() {
        let dir = temp_dir("rename");
        save_conversation(&dir, &sample_conversation("c1", 100)).unwrap();

        assert!(rename_conversation(&dir, "c1", "  Trip plans  ").unwrap());
        let renamed = load_conversation(&dir, "c1").unwrap().unwrap();
        assert_eq!(renamed.title, "Trip plans");
        assert_eq!(renamed.updated_at, 100);

        assert!(rename_conversation(&dir, "c1", "   ").is_err());
        assert!(!rename_conversation(&dir, "missing", "x").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_conversation_removes_the_file_once() {
        let dir = temp_dir("delete");
        save_conversation(&dir, &sample_conversation("c1", 100)).unwrap();

        assert!(delete_conversation(&dir, "c1").unwrap());
        assert_eq!(load_conversation(&dir, "c1").unwrap(), None);
        assert!(!delete_conversation(&dir, "c1").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A model that, while "thinking", runs `during` against the conversations directory — the
    /// stand-in for a hub client renaming or deleting the conversation mid-turn (P78).
    struct ActsMidTurn {
        dir: PathBuf,
        during: fn(&Path),
    }

    #[async_trait::async_trait]
    impl ModelProvider for ActsMidTurn {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<warden_core::tool::ToolSpec>) -> anyhow::Result<warden_core::model::ChatStream> {
            (self.during)(&self.dir);
            Ok(warden_core::model::response_stream(warden_core::model::Response {
                content: "answer".into(),
                tool_calls: Vec::new(),
                usage: None,
            }))
        }
    }

    fn orchestrator_acting_mid_turn(dir: &Path, during: fn(&Path)) -> Orchestrator {
        let vault = Arc::new(Vault::new(dir.join("vault")));
        Orchestrator::new(Arc::new(ActsMidTurn { dir: dir.join("conversations"), during }), vault)
    }

    #[tokio::test]
    async fn handle_turn_keeps_a_rename_made_while_the_model_answered() {
        let root = temp_dir("turn-rename");
        let dir = root.join("conversations");
        save_conversation(&dir, &sample_conversation("c1", 100)).unwrap();
        let orchestrator = orchestrator_acting_mid_turn(&root, |dir| {
            rename_conversation(dir, "c1", "Renamed").unwrap();
        });

        handle_turn(&orchestrator, &dir, "c1", "hi", "hi", Vec::new()).await.unwrap();

        let saved = load_conversation(&dir, "c1").unwrap().unwrap();
        assert_eq!(saved.title, "Renamed");
        assert_eq!(saved.messages.len(), 3);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn handle_turn_does_not_bring_back_a_conversation_deleted_while_the_model_answered() {
        let root = temp_dir("turn-delete");
        let dir = root.join("conversations");
        save_conversation(&dir, &sample_conversation("c1", 100)).unwrap();
        let orchestrator = orchestrator_acting_mid_turn(&root, |dir| {
            delete_conversation(dir, "c1").unwrap();
        });

        let outcome = handle_turn(&orchestrator, &dir, "c1", "hi", "hi", Vec::new()).await.unwrap();

        assert_eq!(outcome.content, "answer");
        assert_eq!(load_conversation(&dir, "c1").unwrap(), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn handle_turn_starts_a_new_conversation_titled_from_the_seed_and_keeps_the_attachments() {
        let root = temp_dir("turn-new");
        let dir = root.join("conversations");
        let orchestrator = orchestrator_acting_mid_turn(&root, |_| {});
        let pdf = Attachment { mime_type: "application/pdf".to_string(), data: "JVBE".to_string() };

        handle_turn(&orchestrator, &dir, "c2", "Plan a trip to Lisbon", "Plan a trip to Lisbon", vec![pdf.clone()]).await.unwrap();

        let saved = load_conversation(&dir, "c2").unwrap().unwrap();
        assert_eq!(saved.title, "Plan a trip to Lisbon");
        assert_eq!(saved.messages.len(), 2);
        assert_eq!(saved.messages[0].attachments, vec![pdf]);
        assert!(saved.messages[1].attachments.is_empty());
        std::fs::remove_dir_all(&root).ok();
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

    fn ssh_entry(id: &str, host: &str, enabled: bool) -> SshHostConfig {
        SshHostConfig {
            id: id.to_string(),
            host: host.to_string(),
            user: "deploy".to_string(),
            port: 22,
            identity_file: None,
            enabled,
            agents: Vec::new(),
            require_approval: false,
        }
    }

    #[test]
    fn ssh_host_entry_defaults_are_safe_when_fields_are_omitted() {
        let config: FileConfig = toml::from_str("[[ssh_hosts]]\nid = \"vps\"\nhost = \"example.com\"\nuser = \"me\"\n").unwrap();

        let host = &config.ssh_hosts[0];
        assert_eq!(host.port, 22);
        assert!(!host.enabled, "a hand-written entry must not be live until switched on");
        assert!(host.agents.is_empty() && host.identity_file.is_none());
        assert!(!host.require_approval, "approval is opt-in per host");
        // And a config with no ssh section at all still parses.
        assert!(toml::from_str::<FileConfig>("enable_shell = true").unwrap().ssh_hosts.is_empty());
    }

    #[test]
    fn build_ssh_tools_only_registers_enabled_and_valid_hosts() {
        let build = |entries: &[SshHostConfig]| build_ssh_tools(entries, std::env::temp_dir(), None);
        assert!(build(&[ssh_entry("off", "example.com", false)]).is_empty());
        assert!(build(&[]).is_empty());
        assert!(build(&[ssh_entry("bad", "-oProxyCommand=evil", true)]).is_empty());

        let tools = build(&[
            ssh_entry("off", "a.example.com", false),
            ssh_entry("bad", "-oProxyCommand=evil", true),
            ssh_entry("web", "b.example.com", true),
        ]);
        let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
        assert_eq!(names, ["ssh_exec", "ssh_upload", "ssh_download"]);
        for tool in &tools {
            assert_eq!(tool.spec().parameters["properties"]["host_id"]["enum"], serde_json::json!(["web"]));
        }
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
    fn resolve_max_delegated_calls_prefers_env_over_file_and_zero_is_a_real_value() {
        assert_eq!(resolve_max_delegated_calls(Some("5".to_string()), Some(1)), 5);
        assert_eq!(resolve_max_delegated_calls(Some("nonsense".to_string()), Some(7)), 7);
        assert_eq!(resolve_max_delegated_calls(None, Some(9)), 9);
        assert_eq!(resolve_max_delegated_calls(None, None), DEFAULT_MAX_DELEGATED_CALLS);
        assert_eq!(resolve_max_delegated_calls(Some("0".to_string()), Some(9)), 0);
    }

    #[test]
    fn resolve_max_parallel_jobs_prefers_env_over_file_and_defaults_to_three() {
        assert_eq!(resolve_max_parallel_jobs(Some("5".to_string()), Some(1)), 5);
        assert_eq!(resolve_max_parallel_jobs(Some("nonsense".to_string()), Some(7)), 7);
        assert_eq!(resolve_max_parallel_jobs(None, Some(2)), 2);
        assert_eq!(resolve_max_parallel_jobs(None, None), 3);
        // `0` is kept as given (the queue itself treats it as one at a time).
        assert_eq!(resolve_max_parallel_jobs(Some("0".to_string()), Some(9)), 0);
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
        let decentralized = build_storage_provider(StorageProviderKind::DecentralizedVault, vault.clone(), None).await.unwrap();
        // Round-trip through the real provider, not just "construction didn't error" — catches a
        // signature-wiring mistake in the `SyncEngine` this arm now builds (P61 follow-up).
        decentralized.write("a.md", b"hello").await.unwrap();
        assert_eq!(decentralized.read("a.md").await.unwrap(), b"hello");
        assert_eq!(decentralized.list().await.unwrap(), vec!["a.md".to_string()]);
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
            can_manage_agents: false,
            allowed_tools: None,
        }
    }

    #[tokio::test]
    async fn delegate_to_agent_targets_use_their_own_tool_list_not_the_chiefs() {
        use warden_core::model::{response_stream, ChatStream, Response};
        use warden_core::tool::ToolSpec;

        /// Answers with the names of the tools it was offered.
        struct EchoesToolNames;
        #[async_trait::async_trait]
        impl ModelProvider for EchoesToolNames {
            async fn chat_stream(&self, _messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                let names = tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join(",");
                Ok(response_stream(Response { content: names, tool_calls: Vec::new(), usage: None }))
            }
        }
        struct Named(&'static str);
        #[async_trait::async_trait]
        impl Tool for Named {
            fn spec(&self) -> ToolSpec {
                ToolSpec { name: self.0.to_string(), description: String::new(), parameters: serde_json::json!({}) }
            }
            async fn call(&self, _args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!("ran"))
            }
        }

        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-delegate-tools-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        let mut orchestrator = Orchestrator::new(Arc::new(EchoesToolNames), vault);
        for name in ["read_file", "write_file", "shell"] {
            orchestrator.register_tool(Arc::new(Named(name)));
        }
        let config = FileConfig {
            agents: vec![
                AgentConfig { allowed_tools: Some(vec!["read_file".into()]), ..agent_config("reader", None) },
                AgentConfig { allowed_tools: Some(vec!["shell".into()]), ..agent_config("ops", None) },
                agent_config("open", None),
            ],
            ..FileConfig::default()
        };

        // The chief is narrowed to `read_file` only *after* the targets are built from the full set.
        let tool = build_delegate_to_agent_tool(&config, &orchestrator).unwrap();
        let _chief = orchestrator.with_allowed_tools(Some(&["read_file".to_string()]));
        let ask = |id: &str| serde_json::json!({ "agent_id": id, "task": "go" });
        assert_eq!(tool.call(ask("reader")).await.unwrap()["result"], "read_file");
        assert_eq!(tool.call(ask("ops")).await.unwrap()["result"], "shell");
        assert_eq!(tool.call(ask("open")).await.unwrap()["result"], "read_file,write_file,shell");
    }

    #[tokio::test]
    async fn an_agent_created_mid_turn_can_be_delegated_to_in_the_same_turn() {
        use warden_core::model::{response_stream, ChatStream, Response, Role, ToolCall};
        use warden_core::tool::{ApprovalRequest, Approver, ToolSpec};

        /// The chief creates "poet", then delegates to it only if the tool now lists it, then repeats the result.
        struct ChiefCreatesThenDelegates;
        #[async_trait::async_trait]
        impl ModelProvider for ChiefCreatesThenDelegates {
            async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                if !tools.iter().any(|t| t.name == "manage_agents") {
                    // The delegation target: it must not be handed the chief's powers.
                    let leaked = tools.iter().any(|t| t.name == "delegate_to_agent");
                    return Ok(response_stream(Response { content: format!("poem (leaked powers: {leaked})"), tool_calls: Vec::new(), usage: None }));
                }
                let call = |name: &str, arguments: serde_json::Value| ToolCall { id: "1".into(), name: name.into(), arguments, thought_signature: None };
                let response = match messages.iter().filter(|m| m.role == Role::Tool).count() {
                    0 => Response {
                        content: String::new(),
                        tool_calls: vec![call("manage_agents", serde_json::json!({ "action": "create", "id": "poet", "persona": "You write poems." }))],
                        usage: None,
                    },
                    1 => {
                        let listed = tools
                            .iter()
                            .find(|t| t.name == "delegate_to_agent")
                            .map(|t| t.parameters["properties"]["agent_id"]["enum"].to_string())
                            .unwrap_or_default();
                        if listed.contains("poet") {
                            Response {
                                content: String::new(),
                                tool_calls: vec![call("delegate_to_agent", serde_json::json!({ "agent_id": "poet", "task": "write" }))],
                                usage: None,
                            }
                        } else {
                            Response { content: format!("poet not offered: {listed}"), tool_calls: Vec::new(), usage: None }
                        }
                    }
                    _ => Response { content: messages.last().unwrap().content.clone(), tool_calls: Vec::new(), usage: None },
                };
                Ok(response_stream(response))
            }
        }
        struct Yes;
        #[async_trait::async_trait]
        impl Approver for Yes {
            async fn approve(&self, _request: ApprovalRequest) -> bool {
                true
            }
        }

        let dir = std::env::temp_dir().join(format!(
            "warden-live-delegate-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let chief_config = AgentConfig { can_delegate_to_agents: true, can_manage_agents: true, ..agent_config("chief", None) };
        let config = FileConfig { agents: vec![chief_config], ..FileConfig::default() };
        save_config(&path, &config).unwrap();

        let vault = Arc::new(Vault::new(dir.join("vault")));
        let orchestrator = Orchestrator::new(Arc::new(ChiefCreatesThenDelegates), vault).with_delegation_limit(10);
        let revision = AgentsRevision::default();
        let delegate = build_live_delegate_to_agent_tool(&path, &config, &orchestrator, revision.clone()).unwrap();
        let manage = ManageAgentsTool::new(&path).with_agents_revision(revision);
        let chief = orchestrator.with_tool(delegate).with_tool(Arc::new(manage)).with_approver(Arc::new(Yes));

        let answer = chief.handle_message(&[], "make me a poet and ask it for a poem").await.unwrap().content;

        assert!(answer.contains("poem (leaked powers: false)"), "{answer}");
        // ...and the agent really was saved, not just faked in memory.
        assert!(load_config_from_path(&path, false).unwrap().agents.iter().any(|a| a.id == "poet"));
    }

    #[tokio::test]
    async fn delegate_to_agent_targets_spend_from_the_turns_delegation_budget() {
        use warden_core::model::{response_stream, ChatStream, Response, Role, ToolCall};
        use warden_core::tool::ToolSpec;

        /// The chief asks its two agents in one round and then repeats what came back; the agents answer.
        struct AsksTwice;
        #[async_trait::async_trait]
        impl ModelProvider for AsksTwice {
            async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                let response = if tools.iter().any(|t| t.name == "delegate_to_agent") {
                    if messages.last().is_some_and(|m| m.role == Role::Tool) {
                        let results: Vec<String> = messages.iter().rev().take_while(|m| m.role == Role::Tool).map(|m| m.content.clone()).collect();
                        Response { content: results.join(" | "), tool_calls: Vec::new(), usage: None }
                    } else {
                        let ask = |id: &str| ToolCall {
                            id: id.into(),
                            name: "delegate_to_agent".into(),
                            arguments: serde_json::json!({ "agent_id": "helper", "task": "go" }),
                            thought_signature: None,
                        };
                        Response { content: String::new(), tool_calls: vec![ask("1"), ask("2")], usage: None }
                    }
                } else {
                    Response { content: "leaf ok".to_string(), tool_calls: Vec::new(), usage: None }
                };
                Ok(response_stream(response))
            }
        }

        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-delegate-budget-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        let orchestrator = Orchestrator::new(Arc::new(AsksTwice), vault).with_delegation_limit(1);
        let config = FileConfig { agents: vec![agent_config("helper", None)], ..FileConfig::default() };
        let chief = orchestrator.with_tool(build_delegate_to_agent_tool(&config, &orchestrator).unwrap());

        let answer = chief.handle_message(&[], "go").await.unwrap().content;

        // One helper call fits in the turn's budget of 1; the second is refused, and the chief still answers.
        assert!(answer.contains("leaf ok"), "{answer}");
        assert!(answer.contains("limit of 1 model calls"), "{answer}");
    }

    #[test]
    fn an_agent_from_a_config_written_before_allowed_tools_keeps_every_tool() {
        let old = "[[agents]]\nid = \"legacy\"\npersona = \"p\"\n";
        let config: FileConfig = toml::from_str(old).unwrap();
        assert_eq!(config.agents[0].allowed_tools, None);
        let new = "[[agents]]\nid = \"tight\"\npersona = \"p\"\nallowed_tools = []\n";
        let config: FileConfig = toml::from_str(new).unwrap();
        assert_eq!(config.agents[0].allowed_tools, Some(Vec::new()));
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

    fn ssh_host(id: &str, agents: &[&str], enabled: bool) -> SshHostConfig {
        SshHostConfig {
            id: id.into(),
            host: "example.com".into(),
            user: "deploy".into(),
            port: 22,
            identity_file: None,
            enabled,
            agents: agents.iter().map(|a| a.to_string()).collect(),
            require_approval: false,
        }
    }

    #[test]
    fn removing_an_agent_prunes_ssh_hosts_and_never_widens_access() {
        let mut config = FileConfig {
            agents: vec![agent_config("ops", None), agent_config("other", None)],
            ssh_hosts: vec![
                ssh_host("only-ops", &["ops"], true),
                ssh_host("shared", &["ops", "other"], true),
                ssh_host("for-other", &["other"], true),
                ssh_host("everyone", &[], true),
            ],
            ..FileConfig::default()
        };

        let effects = remove_agent_references(&mut config, "ops");

        assert_eq!(config.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["other"]);
        // Restricted to the removed agent alone: switched off, not left open to everyone.
        assert_eq!((config.ssh_hosts[0].agents.len(), config.ssh_hosts[0].enabled), (0, false));
        // Shared: just loses the name and stays on.
        assert_eq!((config.ssh_hosts[1].agents.clone(), config.ssh_hosts[1].enabled), (vec!["other".to_string()], true));
        // Unrelated hosts, including the "everyone" one, are untouched.
        assert_eq!(config.ssh_hosts[2], ssh_host("for-other", &["other"], true));
        assert_eq!(config.ssh_hosts[3], ssh_host("everyone", &[], true));
        assert_eq!(
            effects,
            vec![
                SshHostEffect { host_id: "only-ops".into(), switched_off: true },
                SshHostEffect { host_id: "shared".into(), switched_off: false },
            ]
        );
        // An id nobody has changes nothing.
        assert!(remove_agent_references(&mut config, "ghost").is_empty());
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

    // `dedupe_tool_name` itself moved to `warden_core::tool` (shared with `warden-server`'s own
    // collision point, P42) — its unit tests live there now.

    /// A named `Tool` with no real capability, for the `register_mcp_tools` tests below — same
    /// minimal shape as `Named` in `delegate_to_agent_targets_use_their_own_tool_list_not_the_chiefs`.
    struct StubTool(&'static str);
    #[async_trait::async_trait]
    impl Tool for StubTool {
        fn spec(&self) -> warden_core::tool::ToolSpec {
            warden_core::tool::ToolSpec { name: self.0.to_string(), description: String::new(), parameters: serde_json::json!({}) }
        }
        async fn call(&self, _args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!("ran"))
        }
    }

    /// A `ToolProvider` that hands back a fixed list — what makes `register_mcp_tools` testable
    /// without a real MCP process (P46): it's generic over the trait, not tied to `McpToolProvider`.
    struct FixedProvider(Vec<Arc<dyn Tool>>);
    #[async_trait::async_trait]
    impl ToolProvider for FixedProvider {
        async fn tools(&self) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn register_mcp_tools_keeps_the_bare_name_when_nothing_collides() {
        let mut base_tools: Vec<Arc<dyn Tool>> = vec![Arc::new(StubTool("read_file"))];
        register_mcp_tools(&mut base_tools, "anchor", Ok(FixedProvider(vec![Arc::new(StubTool("search"))]))).await;

        let names: Vec<String> = base_tools.iter().map(|t| t.spec().name).collect();
        assert_eq!(names, vec!["read_file".to_string(), "search".to_string()]);
    }

    #[tokio::test]
    async fn register_mcp_tools_renames_on_collision_and_both_stay_reachable() {
        let mut base_tools: Vec<Arc<dyn Tool>> = vec![Arc::new(StubTool("search"))];
        register_mcp_tools(&mut base_tools, "anchor", Ok(FixedProvider(vec![Arc::new(StubTool("search"))]))).await;

        let names: Vec<String> = base_tools.iter().map(|t| t.spec().name).collect();
        assert_eq!(names, vec!["search".to_string(), "anchor__search".to_string()]);
        // Not just renamed in the spec — the renamed tool still forwards `call` to the real one.
        assert_eq!(base_tools[1].call(serde_json::Value::Null).await.unwrap(), serde_json::json!("ran"));
    }

    #[tokio::test]
    async fn register_mcp_tools_leaves_base_tools_untouched_on_a_failed_connection() {
        let mut base_tools: Vec<Arc<dyn Tool>> = vec![Arc::new(StubTool("read_file"))];
        register_mcp_tools::<FixedProvider>(&mut base_tools, "anchor", Err(anyhow::anyhow!("connection refused"))).await;

        assert_eq!(base_tools.len(), 1);
    }
}
