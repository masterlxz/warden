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
use warden_core::memory::Vault;
use warden_core::model::anthropic::AnthropicProvider;
use warden_core::model::gemini::GeminiProvider;
pub use warden_core::model::key_check::KeyCheck;
use warden_core::model::labeled::Labeled;
use warden_core::model::openai::OpenAiProvider;
use warden_core::model::{Attachment, FallbackProvider, Message, ModelProvider, Usage};
use warden_core::orchestrator::{MessageOutcome, Orchestrator};
use warden_core::tool::delegate::DelegateTool;
use warden_core::tool::delegate_to_agent::{AgentResolver, AgentsRevision, DelegateToAgentTool, DelegationSpawner, NamedSubAgent};
use warden_core::tool::delegate::ModelChoices;
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

pub mod agent_changes;
pub mod agent_scope;
pub mod agent_tasks;
pub mod auto_sync;
pub mod history;
pub mod bot_access;
pub mod activity;
pub mod bot_hub;
pub mod bot_outbox;
pub mod bot_pairing;
pub mod code_turn;
pub mod learning;
pub mod machine_settings;
mod config_file;
pub mod manage_agents;
pub mod manage_tasks;
pub mod member_crypto;
pub mod org;
pub mod org_edit;
pub mod outreach;
pub mod recovery;
pub mod risk;
pub mod node_model;
pub mod message_agent;
pub mod project_scope;
pub mod saved_hubs;
pub mod settings;
pub mod skill_gen;
pub mod spend;
pub mod tasks;
pub mod webhooks;
pub mod usage;
pub mod users;
pub use agent_scope::{scope_to_agent, AgentExtras, ScopedAgent};
pub use config_file::render_config;
pub use manage_agents::ManageAgentsTool;
pub use manage_tasks::ManageTasksTool;
pub use outreach::{MessageUserTool, OutreachConfig};
pub use message_agent::{channel_id, ConversationsChanged, MessageAgentTool, CHANNEL_PREFIX};
pub use project_scope::{check_node_path, node_folder, node_folder_ref, scope_to_project, scope_to_workdir, WITHHELD_IN_A_PROJECT};
pub use tasks::TaskConfig;
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
    /// A model on a node (P93): `ProviderConfig::node` names the node, `model` its provider there.
    /// Only answers through a hub with that node connected.
    Node,
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
    /// which has no universal default (depends entirely on what's hosted there). For
    /// `Provider::Node`, the id of the provider *on that node*.
    pub model: Option<String>,
    /// Only for `Provider::Node` (P93): the device id of the node whose model this is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
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
    /// Opt-in (P46, "funcionários" mode) for the `message_agent` tool: leave a message in a
    /// conversation with another agent, who answers it there in the background. Switched on by a
    /// person only — `manage_agents` never grants it.
    #[serde(default)]
    pub can_message_agents: bool,
    /// Opt-in (P92) for the `manage_tasks` tool: list, create, edit and delete scheduled tasks, every
    /// change waiting for the person's yes. Switched on by a person only — `manage_agents` never grants it.
    #[serde(default)]
    pub can_manage_tasks: bool,
    /// Tool isolation (P46): the only tools this agent may use, by name. `None` (the default, and
    /// what every config.toml written before this field means) keeps every tool. `delegate_to_agent`
    /// and `manage_agents` never belong here — they follow the two `can_*` flags above. Applied in
    /// code (`Orchestrator::with_allowed_tools`), so it holds however the agent is reached: as the
    /// conversation's agent or as a `delegate_to_agent` target.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// How much this agent may do without asking (P122), 1 to 4: 1 only answers, 2 suggests (a call that would change
    /// something is refused), 3 asks a person before every such call, 4 runs its tools on its own. 4 is what every
    /// agent written before this field did, so it is the default. Applied in code (`Orchestrator::with_autonomy`),
    /// and a delegate target never gets more than the agent that called it.
    #[serde(default = "default_autonomy")]
    pub autonomy: u8,
    /// The kinds of action this agent must have a person approve even at autonomy 4 (P122): which tool calls belong to
    /// which category is the `risk` module's. Empty (the default) asks for none, as an agent always did. Applied in
    /// code (`Orchestrator::with_approval_rules`), and a delegate target asks for these plus its caller's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_required: Vec<warden_core::autonomy::Category>,
    /// P120: this agent's role in the organization ("Head of engineering"), free text. Only shown for now — it changes
    /// nothing an agent may do. The owner's agents only: a member's never has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// P120: the id of the agent this one reports to, so the organization is a tree (`org::check_hierarchy`: no one
    /// reports to themselves, to a stranger or in a circle). Only shown for now; the owner's agents only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reports_to: Option<String>,
    /// P84: the workspace member this agent belongs to — `None` is the owner's. A member's agent is
    /// only theirs: nobody else sees it or talks to it, and it never gets the `can_*` flags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// P84: the members the owner shares this agent with, by username, or `"*"` for everyone. Only
    /// meaningful on the owner's agents; empty keeps it the owner's alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_with: Vec<String>,
    /// P123: the models this agent may pick for the tasks it delegates, by provider, combo or policy id. Empty (the default) leaves
    /// the choice open, as before. Otherwise only these are offered, and the **first** one is what a delegation that names no `model`
    /// gets — so a list of one is the person dictating the model of every task this agent hands out. Set by a person only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delegation_models: Vec<String>,
}

/// `AgentConfig.autonomy` of an agent that doesn't say (P122): 4, no change from before the field existed.
pub fn default_autonomy() -> u8 {
    warden_core::autonomy::Autonomy::DEFAULT_LEVEL
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

/// What agents may do with one node (P93, TOML `[[nodes]]`) — the hub's half of the two locks; the
/// node's own operator chose what it offers (`warden-server node --shell/--files`). Keyed by the
/// node's device id. Like `SshHostConfig`, off unless a person switches it on.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NodeAccessConfig {
    /// The node's device id, as the hub's device list shows it.
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    /// Agents allowed to use it. Empty means every agent and every channel without one. A name
    /// that no longer matches an agent matches nobody, so a stale list only ever closes access.
    #[serde(default)]
    pub agents: Vec<String>,
    /// Ask a human before every command or file operation on this node; a channel that can't ask
    /// (a scheduled task, Telegram) refuses instead.
    #[serde(default)]
    pub require_approval: bool,
}

/// One named combo (P90): `providers` are ids from `FileConfig::providers`, tried in order — the
/// first that isn't down answers.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ComboConfig {
    pub id: String,
    pub providers: Vec<String>,
}

/// One named model policy (P123): a word a delegating agent can say instead of a provider id — "fast", "cheap", "reasoning",
/// "code", "multimodal" — that `model` (a provider or a combo) answers. `description` is what the agent reads to know when to
/// pick it. Shares one namespace with providers and combos.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelPolicyConfig {
    pub id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Config for `warden_sync::GitSyncEngine` (P63, v1) — a self-hosted/remote git repo (Gitea,
/// GitHub, ...) as an alternative to Arweave/TruthID for syncing the vault, for whoever doesn't
/// want that dependency. HTTPS + token only in v1 (SSH/deploy-key is v2); the token is only ever
/// read here and passed to `git` as a per-invocation URL credential — `GitSyncEngine` never writes
/// it to disk. Set from the desktop's and the web's Settings (the web only takes `https://`).
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
    /// `id` of the `providers` entry — or of a `combos` entry (P90) — to use. Ignored (and
    /// unnecessary) while `providers` is empty and the legacy fallback is in play.
    pub active_provider: Option<String>,
    /// Named routing combos (P90): pick one wherever a provider can be picked, and it tries its
    /// providers in order when one is down (P79's `FallbackProvider`). Ids share one namespace
    /// with `providers`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub combos: Vec<ComboConfig>,
    /// Named model policies (P123): "fast", "cheap", "reasoning"... each answered by a provider or combo, offered to an agent
    /// that delegates alongside the plain ids. No UI — config.toml only, same posture as `delegate_max_depth`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_policies: Vec<ModelPolicyConfig>,
    /// P79's global reserve list, from before the combos replaced it (Sessão 105). Read and
    /// turned into a combo on load (`migrate_legacy_fallbacks`), never written back.
    #[serde(default, skip_serializing, rename = "fallback_providers")]
    pub legacy_fallback_providers: Vec<String>,
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
    /// `storage_provider = "..."` and `[remote_node]` from before Sessão 105, when P61 had the
    /// vault's memory live on a chosen backend (the desktop wrote the first on every save). The
    /// memory is always local now and only syncs, so both are read and ignored — the file has
    /// `deny_unknown_fields` and would otherwise stop loading — and never written back, so the
    /// next save drops them.
    #[serde(default, skip_serializing, rename = "storage_provider")]
    pub legacy_storage_provider: Option<toml::Value>,
    #[serde(default, skip_serializing, rename = "remote_node")]
    pub legacy_remote_node: Option<toml::Value>,
    /// Sync via a remote git repo instead of Arweave/TruthID (P63) — `None` means this backend
    /// isn't configured.
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
    /// Scheduled tasks (P92, TOML `[[tasks]]`): a prompt an agent runs on its own, on a schedule.
    /// Only a hub started with `--run-tasks` runs them — the file syncs, so every hub reads them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskConfig>,
    /// The agents a person allowed to start messages (P121, TOML `[[outreach]]`): each gets the `message_user` tool, which writes in its own
    /// channel, and may also forward the message to external channels. An agent with no entry cannot message the person first. Edited
    /// in `config.toml` by the person; no tool writes it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outreach: Vec<OutreachConfig>,
    /// Incoming webhooks (P105, TOML `[[webhooks]]`): a prompt an agent runs when someone `POST`s to the hub's
    /// `/hooks/<id>` with the webhook's token. The tokens aren't here (they don't sync): see `webhooks`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub webhooks: Vec<webhooks::WebhookConfig>,
    /// Which risk category a tool's calls belong to (P122, TOML `[[tool_categories]]`), for the tools Warden doesn't
    /// know (an MCP server's). What an agent's `approval_required` is checked against; see the `risk` module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_categories: Vec<risk::ToolCategoryConfig>,
    /// What agents may do with each node (P93, TOML `[[nodes]]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeAccessConfig>,
    /// The members of the workspace (P84, TOML `[[users]]`) — the root isn't listed, it's whoever
    /// holds the pairing key. See the `users` module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<users::UserConfig>,
    /// Members the owner removed whose data is encrypted (P84 fatia 4, TOML `[[removed_users]]`): their
    /// entry — and with it the wrapped key — is kept, so the data left on disk can still be opened
    /// by them (`users restore`) instead of being lost for good. `users purge` ends it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_users: Vec<users::UserConfig>,
    /// Who besides a person may open their encrypted data (P84 fatia 4 parte B, TOML
    /// `recovery_policy`): `private` (nobody, the default), `consent` (the owner and the person's
    /// recovery code together) or `company` (the owner alone, recorded). See `recovery`.
    #[serde(default, skip_serializing_if = "recovery::RecoveryPolicy::is_private")]
    pub recovery_policy: recovery::RecoveryPolicy,
    /// The public half of the owner's recovery key, in hex — what members' data is prepared with.
    /// The private half is shown once and never kept here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_public_key: Option<String>,
    /// Folders of the owner's vault shared with members (P84 fatia 3, TOML `[[spaces]]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spaces: Vec<users::SpaceConfig>,
    /// Which chain the TruthID registry is read from, when a member links a TruthID (P84 fatia 5,
    /// TOML `truthid_network`: `base-mainnet` or `base-sepolia`).
    #[serde(default, skip_serializing_if = "is_default_network")]
    pub truthid_network: warden_truthid::identity::Network,
    /// The JSON-RPC endpoint for that chain; the network's public one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truthid_rpc_url: Option<String>,
    /// The public `https://` address of this hub as the TruthID app reaches it (P113, TOML
    /// `truthid_public_url`, e.g. `https://hub.tailnet.ts.net`): the app only posts a login to an `https://`
    /// URL with a valid certificate. Without it, signing in with a TruthID is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truthid_public_url: Option<String>,
    /// The assistant learning from conversations (P104, TOML `[learning]`): off unless turned on. See `learning`.
    #[serde(default, skip_serializing_if = "learning::LearningSettings::is_default")]
    pub learning: learning::LearningSettings,
    /// Who may talk to the Telegram bot (P117, TOML `[telegram]`): nobody until listed. See `bot_access`.
    #[serde(default, skip_serializing_if = "bot_access::TelegramSettings::is_default")]
    pub telegram: bot_access::TelegramSettings,
    /// Who may talk to the WhatsApp bot (P117, TOML `[whatsapp]`): nobody until listed. See `bot_access`.
    #[serde(default, skip_serializing_if = "bot_access::WhatsAppSettings::is_default")]
    pub whatsapp: bot_access::WhatsAppSettings,
    /// The hub the bots ask on behalf of a member (P117, TOML `[bot_hub]`): without it every chat is
    /// answered as the owner. See `bot_access` and `bot_hub`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_hub: Option<bot_access::BotHubSettings>,
}

fn is_default_network(network: &warden_truthid::identity::Network) -> bool {
    *network == warden_truthid::identity::Network::default()
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
        Provider::OpenaiCompatible | Provider::Node => None,
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
    for combo in &mut config.combos {
        for id in &mut combo.providers {
            if id == old_id {
                *id = new_id.to_string();
            }
        }
    }
    repoint_policies(config, old_id, new_id);
}

/// A model policy (P123) that answered with `old_id` answers with `new_id` now, and so does an agent's list of the models it may
/// delegate with.
fn repoint_policies(config: &mut FileConfig, old_id: &str, new_id: &str) {
    for policy in &mut config.model_policies {
        if policy.model == old_id {
            policy.model = new_id.to_string();
        }
    }
    for agent in &mut config.agents {
        for model in agent.delegation_models.iter_mut().filter(|m| m.as_str() == old_id) {
            *model = new_id.to_string();
        }
    }
}

/// Takes out of every agent's list of the models it may delegate with (P123) what is no longer a provider, a combo or a policy — after a
/// save that edited those lists, so a name a person removed there doesn't linger in an agent. A list left empty means the choice is open.
pub fn prune_delegation_models(config: &mut FileConfig) {
    let known: Vec<String> = config.providers.iter().map(|p| p.id.clone()).chain(config.combos.iter().map(|c| c.id.clone())).chain(config.model_policies.iter().map(|p| p.id.clone())).collect();
    for agent in &mut config.agents {
        agent.delegation_models.retain(|m| known.contains(m));
    }
}

/// `id` (a provider, combo or policy) is gone: no agent may delegate with it any more. A list left empty means the choice is open
/// again, which is what the person gets when the one model they had limited it to is removed.
fn forget_delegation_model(config: &mut FileConfig, id: &str) {
    for agent in &mut config.agents {
        agent.delegation_models.retain(|m| m != id);
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
    for combo in &mut config.combos {
        combo.providers.retain(|id| id != removed_id);
    }
    // A combo left with nothing to try goes too, and so does whatever pointed at it.
    let emptied: Vec<String> = config.combos.iter().filter(|c| c.providers.is_empty()).map(|c| c.id.clone()).collect();
    for id in emptied {
        remove_combo(config, &id);
    }
    // A policy that answered with it goes too, and with the policy, whatever listed it.
    let gone: Vec<String> = config.model_policies.iter().filter(|p| p.model == removed_id).map(|p| p.id.clone()).collect();
    config.model_policies.retain(|p| p.model != removed_id);
    forget_delegation_model(config, removed_id);
    for id in gone {
        forget_delegation_model(config, &id);
    }
}

/// `rename_provider_cascade` for a combo (P90): the active model and every agent that named it.
pub fn rename_combo(config: &mut FileConfig, old_id: &str, new_id: &str) {
    for combo in &mut config.combos {
        if combo.id == old_id {
            combo.id = new_id.to_string();
        }
    }
    if config.active_provider.as_deref() == Some(old_id) {
        config.active_provider = Some(new_id.to_string());
    }
    for agent in &mut config.agents {
        if agent.provider_id.as_deref() == Some(old_id) {
            agent.provider_id = Some(new_id.to_string());
        }
    }
    repoint_policies(config, old_id, new_id);
}

/// Removes a combo and every reference to it (P90).
pub fn remove_combo(config: &mut FileConfig, id: &str) {
    config.combos.retain(|c| c.id != id);
    let gone: Vec<String> = config.model_policies.iter().filter(|p| p.model == id).map(|p| p.id.clone()).collect();
    config.model_policies.retain(|p| p.model != id);
    forget_delegation_model(config, id);
    for policy in gone {
        forget_delegation_model(config, &policy);
    }
    if config.active_provider.as_deref() == Some(id) {
        config.active_provider = None;
    }
    for agent in &mut config.agents {
        if agent.provider_id.as_deref() == Some(id) {
            agent.provider_id = None;
        }
    }
}

/// P79's global reserve list (`fallback_providers`) as the combo that replaced it (P90): with a
/// provider active, `"<active>-reserva"` = the active one then the list, and it becomes the
/// active model — so the file keeps doing what it did. The next save writes the combo instead.
fn migrate_legacy_fallbacks(config: &mut FileConfig) {
    let legacy = std::mem::take(&mut config.legacy_fallback_providers);
    let Some(active) = config.active_provider.clone().filter(|id| config.providers.iter().any(|p| &p.id == id)) else {
        return;
    };
    if legacy.is_empty() {
        return;
    }
    let mut providers = vec![active.clone()];
    for id in legacy {
        if !providers.contains(&id) && config.providers.iter().any(|p| p.id == id) {
            providers.push(id);
        }
    }
    let mut combo_id = format!("{active}-reserva");
    while config.providers.iter().any(|p| p.id == combo_id) || config.combos.iter().any(|c| c.id == combo_id) {
        combo_id.push('-');
    }
    config.combos.push(ComboConfig { id: combo_id.clone(), providers });
    config.active_provider = Some(combo_id);
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
    // P120: the ones that reported to it report to its superior from now on.
    org::reparent_reports(agents, agent_id);
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
    forget_agent_in_nodes(&mut config.nodes, agent_id);
    remove_agent_from(&mut config.agents, &mut config.ssh_hosts, agent_id)
}

/// Drops a removed agent from every node's list (P93). A node left with no agent is switched off,
/// never opened to everyone — the same rule as the SSH hosts.
pub fn forget_agent_in_nodes(nodes: &mut [NodeAccessConfig], agent_id: &str) {
    for node in nodes.iter_mut().filter(|n| n.agents.iter().any(|a| a == agent_id)) {
        node.agents.retain(|a| a != agent_id);
        if node.agents.is_empty() {
            node.enabled = false;
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
    /// Names of the tools the assistant's turn ran (P115), so the learning step can tell work from a claim.
    /// Absent on older conversations and on turns without tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_used: Vec<String>,
}

/// The message a thread started from (P125): the conversation it is in and the id of the message. A thread is a conversation of its own
/// with this link; it works in the folder and project of the conversation it came from, and its turns see that conversation up to
/// this message.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadParent {
    pub conversation_id: String,
    pub message_id: String,
}

/// How many messages of the conversation a thread's model sees before the thread itself (P125): the last ones that end at the message
/// the thread started from. A conversation has no limit of its own, but a thread is a side question and shouldn't resend a long one.
pub const THREAD_CONTEXT_MAX: usize = 40;

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
    /// The project this conversation belongs to (P103), set when it is created and never changed by a later
    /// turn. A project that is gone (its `PROJECT.md` removed) is read as "no project".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The code engine's session for this conversation (P103 b): a conversation of a code project is a session of the
    /// opencode, and what it said and did lives there; this is how the next message finds it again. Moving the
    /// conversation to another project forgets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_session_id: Option<String>,
    /// The folder of the machine this conversation works in (P102), chosen before its first message and never
    /// changed after: `read_file`, `write_file` and the shell act there. Only a conversation in no project has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Set when this conversation is a thread (P125): the message it was started from, fixed when it is created. A conversation
    /// that isn't one has none, and a thread can't be the parent of another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ThreadParent>,
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
    let on_disk = match member_crypto::dir_state(dir) {
        member_crypto::DirState::Plain => contents.into_bytes(),
        member_crypto::DirState::Unlocked(cipher) => cipher.seal(contents.as_bytes()),
        member_crypto::DirState::Locked => anyhow::bail!(warden_core::memory::LOCKED_MESSAGE),
    };
    std::fs::write(&path, on_disk).with_context(|| format!("failed to write conversation file at {}", path.display()))
}

/// What a conversation file holds, decrypted when the folder is a member's encrypted one. A plain
/// file in an encrypted folder is read as it is: it's one the migration hasn't reached yet.
fn conversation_text(dir: &Path, bytes: Vec<u8>) -> anyhow::Result<String> {
    let bytes = match member_crypto::dir_state(dir) {
        member_crypto::DirState::Locked => anyhow::bail!(warden_core::memory::LOCKED_MESSAGE),
        member_crypto::DirState::Unlocked(cipher) if warden_core::memory::VaultCipher::is_sealed(&bytes) => cipher.open(&bytes)?,
        _ => bytes,
    };
    Ok(String::from_utf8(bytes)?)
}

/// Lists every persisted conversation, newest-updated first. A directory that doesn't exist yet
/// just means "no conversations saved" — not an error, mirroring `load_config_from_path`'s
/// non-required case. A file that fails to parse is skipped rather than failing the whole list,
/// so one corrupt conversation can't make every other one disappear from the sidebar.
pub fn list_conversations(dir: &Path) -> anyhow::Result<Vec<Conversation>> {
    if matches!(member_crypto::dir_state(dir), member_crypto::DirState::Locked) {
        anyhow::bail!(warden_core::memory::LOCKED_MESSAGE);
    }
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
        if let Ok(contents) = std::fs::read(&path).map_err(anyhow::Error::from).and_then(|bytes| conversation_text(dir, bytes)) {
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
    match std::fs::read(&path) {
        Ok(bytes) => {
            let contents = conversation_text(dir, bytes).with_context(|| format!("failed to read conversation file at {}", path.display()))?;
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

/// Where `warden-server` keeps its scheduled tasks' conversations and run state (P92). Not under
/// `default_server_conversations_dir`, whose subdirectories are device ids a client picks.
pub fn default_server_tasks_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("tasks-server"))
}

/// Where `warden-server`'s persistent device pairing registry lives (Fase 9.3) — same
/// `dirs::config_dir()` base as `default_server_conversations_dir`, not overridable via CLI yet
/// (same posture that function already has).
pub fn default_server_devices_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("devices.json"))
}

/// Where a hub keeps the Warden API's keys (P12) — only their hashes, never a key itself. Shared by
/// the standalone hub and the desktop's embedded one, like `devices.json`.
pub fn default_api_keys_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("api_keys.json"))
}

/// Where a hub keeps its webhooks' tokens (P105) — only their hashes, and outside the synced config: a token belongs to
/// the hub whose URL the caller knows, and only that one runs the call.
pub fn default_webhook_tokens_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("webhook_tokens.json"))
}

/// Where a hub keeps the TLS certificate it fetches with `tailscale cert` (P36) — shared by
/// `warden-server serve --tailscale-cert` and the desktop's embedded hub.
pub fn default_tls_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("tls"))
}

/// What the desktop's Workspace screen embeds in the QR code a new client scans (Fase 9.7) — the
/// two fields the mobile `ConnectionScreen` would otherwise need typed by hand: which hub to
/// connect to, and its shared secret. Deliberately its own tiny JSON file, not a field on
/// `FileConfig`: it's a Workspace-only concern (generating a QR), unrelated to the
/// providers/agents/mcp settings that file's Settings-screen form already covers, and it never
/// gets read by `bootstrap()`.
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

/// Where the hub logs every call its agents make on a node (P93) — same format as the SSH log.
pub fn default_node_audit_log_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("node_audit.jsonl"))
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
    handle_agent_turn(orchestrator, conversations_dir, conversation_id, title_seed, user_input, attachments, None, None, None).await
}

/// The named agent a `handle_agent_turn` speaks as: its persona goes in as the system prompt, and
/// its id is saved on the conversation so a client reopening it picks the same agent (P46, hub).
#[derive(Debug, Clone, Copy)]
pub struct TurnAgent<'a> {
    pub id: &'a str,
    pub persona: &'a str,
}

/// `handle_turn` with an optional agent (P46): `orchestrator` should already be scoped to it
/// (`agent_scope::scope_to_agent`). `None` is exactly `handle_turn` — no persona, and the
/// conversation's `agent_id` is cleared, since a client that sends none has no agent selected.
///
/// `project` (P103) is the project a conversation *starts* in: it is saved on the conversation when this call creates
/// it (an unknown project is an error, nothing is run). A conversation that already exists runs in the project it was
/// created in, whatever is sent; one whose project was removed since runs as an ordinary conversation.
///
/// `workdir` (P102) is the folder of this machine a conversation in no project *starts* in, with the same rule: saved
/// when this call creates the conversation, ignored when it already exists (it keeps its own) or when it is in a
/// project. The caller has already checked that the person may use it.
#[allow(clippy::too_many_arguments)]
pub async fn handle_agent_turn(
    orchestrator: &Orchestrator,
    conversations_dir: &Path,
    conversation_id: &str,
    title_seed: &str,
    user_input: &str,
    attachments: Vec<Attachment>,
    agent: Option<TurnAgent<'_>>,
    project: Option<&str>,
    workdir: Option<&str>,
) -> anyhow::Result<MessageOutcome> {
    handle_agent_turn_in(orchestrator, conversations_dir, conversation_id, title_seed, user_input, attachments, agent, project, workdir, None).await
}

/// An ordinary conversation id: 1 to 64 letters, digits, `_` or `-` (what the hub accepts), never a path.
fn is_conversation_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The last messages of `anchor` that end at message `message_id` (P125): what a thread's model sees of the conversation it came from.
/// Empty when that message isn't there.
fn thread_context<'a>(anchor: &'a Conversation, message_id: &str) -> &'a [ConversationMessage] {
    let Some(at) = anchor.messages.iter().position(|m| m.id == message_id) else { return &[] };
    let end = at + 1;
    &anchor.messages[end.saturating_sub(THREAD_CONTEXT_MAX)..end]
}

/// `handle_agent_turn` with the thread a conversation *starts* as (P125): `thread_of` is the message of another conversation this one is a
/// thread of. Like `project`, it is only read when this call creates the conversation (an unknown message, or a thread as the parent, is an
/// error and nothing is run); one that already exists keeps its own. A thread works in the folder and the project of the conversation it
/// came from, whatever is sent, and its model sees that conversation up to the message (`THREAD_CONTEXT_MAX` at most) and then the thread.
#[allow(clippy::too_many_arguments)]
pub async fn handle_agent_turn_in(
    orchestrator: &Orchestrator,
    conversations_dir: &Path,
    conversation_id: &str,
    title_seed: &str,
    user_input: &str,
    attachments: Vec<Attachment>,
    agent: Option<TurnAgent<'_>>,
    project: Option<&str>,
    workdir: Option<&str>,
    thread_of: Option<&ThreadParent>,
) -> anyhow::Result<MessageOutcome> {
    let existing = load_conversation(conversations_dir, conversation_id)?;
    let existed = existing.is_some();
    // P125: the message this is a thread of, from the file when it exists and from the request when it is being created.
    let parent: Option<ThreadParent> = match &existing {
        Some(conversation) => conversation.parent.clone(),
        None => thread_of.cloned(),
    };
    if let (false, Some(link)) = (existed, &parent) {
        // The id of the conversation it came from is a file name: only an ordinary id is looked up, so a client that skipped the hub's check
        // can't make this read another folder. Checked before anything is read.
        anyhow::ensure!(is_conversation_id(&link.conversation_id), "'{}' isn't a conversation id", link.conversation_id);
    }
    let anchor = match &parent {
        Some(link) => load_conversation(conversations_dir, &link.conversation_id)?,
        None => None,
    };
    if let (false, Some(link)) = (existed, &parent) {
        let anchor = anchor.as_ref().ok_or_else(|| anyhow::anyhow!("there is no conversation '{}' to start a thread in", link.conversation_id))?;
        anyhow::ensure!(link.conversation_id != conversation_id, "a conversation can't be a thread of itself");
        anyhow::ensure!(anchor.parent.is_none(), "a thread can't have a thread of its own");
        anyhow::ensure!(anchor.messages.iter().any(|m| m.id == link.message_id), "there is no message '{}' in that conversation", link.message_id);
    }
    // A thread starts in the folder and the project of the conversation it came from.
    let (project, workdir) = match (&anchor, existed) {
        (Some(from), false) => (from.project_id.as_deref(), from.workdir.as_deref()),
        _ => (project, workdir),
    };
    let mut history: Vec<Message> = Vec::new();
    if let (Some(link), Some(from)) = (&parent, &anchor) {
        history.extend(thread_context(from, &link.message_id).iter().map(to_message));
    }
    history.extend(existing.iter().flat_map(|c| &c.messages).map(to_message));
    // P103: a conversation's project is the one it was created in; what a client sends later doesn't move it.
    let project_id = match &existing {
        Some(conversation) => conversation.project_id.clone(),
        None => project.map(str::to_string),
    };
    // P102: the same for the working folder, which only a conversation in no project has. A folder that is gone is an
    // error and not a silent fall back to the vault: the person chose it.
    let workdir = match &existing {
        Some(conversation) => conversation.workdir.clone(),
        None => workdir.map(str::to_string),
    }
    .filter(|_| project_id.is_none());
    let scoped = match (&project_id, &workdir) {
        (Some(id), _) => scope_to_project(orchestrator, id)?,
        // A folder on a node is set up by the hub before this call (it needs the hub's node tools); here it is only
        // saved. A channel that has no way to do that must not run the turn as if it had no folder.
        (None, Some(folder)) if node_folder(folder).is_some() => {
            anyhow::ensure!(orchestrator.has_briefing(), "this conversation works in a folder on a node, which only the hub can set up");
            None
        }
        (None, Some(folder)) => Some(scope_to_workdir(orchestrator, folder)?),
        (None, None) => None,
    };
    if !existed && project.is_some() && scoped.is_none() {
        anyhow::bail!("there is no project '{}'", project.unwrap_or_default());
    }
    let outcome = scoped.as_ref().unwrap_or(orchestrator).handle_turn(&history, user_input, attachments.clone(), agent.map(|a| a.persona)).await?;

    let user = ConversationMessage {
        id: message_id(),
        role: ChatRole::User,
        content: user_input.to_string(),
        created_at: now_millis(),
        usage: None,
        attachments,
        generated_files: Vec::new(),
        tools_used: Vec::new(),
    };
    let options = AppendOptions {
        title_seed,
        agent_id: agent.map(|a| a.id),
        project_id: project_id.as_deref().filter(|_| !existed),
        workdir: workdir.as_deref().filter(|_| !existed),
        thread_of: parent.as_ref().filter(|_| !existed),
        create: !existed,
        ..Default::default()
    };
    append_messages(conversations_dir, conversation_id, options, vec![user, assistant_message(&outcome)])?;
    Ok(outcome)
}

fn assistant_message(outcome: &MessageOutcome) -> ConversationMessage {
    ConversationMessage {
        id: message_id(),
        role: ChatRole::Assistant,
        content: outcome.content.clone(),
        created_at: now_millis(),
        usage: outcome.usage,
        attachments: outcome.attachments.clone(),
        generated_files: outcome.generated_files.clone(),
        tools_used: outcome.tools_used.clone(),
    }
}

/// How `append_messages` treats the conversation besides adding the messages.
#[derive(Debug, Clone, Copy, Default)]
pub struct AppendOptions<'a> {
    /// The title of a conversation this call creates.
    pub title_seed: &'a str,
    /// Always set: `None` clears it (whoever sent the turn had no agent selected).
    pub agent_id: Option<&'a str>,
    /// `None` leaves it as it is; `Some(None)` clears it. Only the desktop tracks one.
    pub provider_id: Option<Option<&'a str>>,
    /// The project of a conversation this call creates (P103). Unlike `agent_id`, a conversation that already exists
    /// keeps the one it has whatever is sent here: every client that doesn't know projects sends none, and one of
    /// them must not take a conversation out of its project by talking in it.
    pub project_id: Option<&'a str>,
    /// The code engine's session (P103 b): `None` leaves it as it is, like `provider_id`'s outer `None`.
    pub engine_session_id: Option<&'a str>,
    /// The working folder of a conversation this call creates (P102): like `project_id`, an existing conversation
    /// keeps the one it has, and a conversation in a project has none.
    pub workdir: Option<&'a str>,
    /// The message a conversation this call creates is a thread of (P125): like `project_id`, only read on creation.
    pub thread_of: Option<&'a ThreadParent>,
    /// Create the file when it's missing. Otherwise a missing file was deleted meanwhile and stays
    /// deleted.
    pub create: bool,
}

/// Appends `messages` to a conversation file and saves it, returning it as saved (`None`: missing
/// and not `create`). The file is read here, under the lock `rename_conversation`/
/// `delete_conversation` take (P78): a model call can take a minute, and in the meantime a client
/// may have renamed or deleted the conversation, or another writer — an agent answering a
/// `message_agent` note (P46), the desktop's own screen, the CLI in another process — added to
/// it, so a copy loaded before must never be saved over what changed.
pub fn append_messages(dir: &Path, id: &str, options: AppendOptions<'_>, messages: Vec<ConversationMessage>) -> anyhow::Result<Option<Conversation>> {
    let _guard = ConversationWriteGuard::acquire(dir)?;
    let mut conversation = match load_conversation(dir, id)? {
        Some(conversation) => conversation,
        None if !options.create => return Ok(None),
        None => {
            let now = now_millis();
            Conversation {
                id: id.to_string(),
                title: title_from(options.title_seed),
                messages: Vec::new(),
                created_at: now,
                updated_at: now,
                agent_id: None,
                provider_id: None,
                project_id: options.project_id.map(str::to_string),
                engine_session_id: None,
                workdir: options.workdir.filter(|_| options.project_id.is_none()).map(str::to_string),
                parent: options.thread_of.cloned(),
            }
        }
    };
    conversation.messages.extend(messages);
    conversation.agent_id = options.agent_id.map(str::to_string);
    if let Some(session) = options.engine_session_id {
        conversation.engine_session_id = Some(session.to_string());
    }
    if let Some(provider_id) = options.provider_id {
        conversation.provider_id = provider_id.map(str::to_string);
    }
    conversation.updated_at = now_millis();
    save_conversation(dir, &conversation)?;
    Ok(Some(conversation))
}

fn append_to_conversation(
    dir: &Path,
    id: &str,
    title_seed: &str,
    agent_id: Option<&str>,
    project_id: Option<&str>,
    create: bool,
    messages: Vec<ConversationMessage>,
) -> anyhow::Result<bool> {
    let options = AppendOptions { title_seed, agent_id, provider_id: None, project_id, create, ..Default::default() };
    Ok(append_messages(dir, id, options, messages)?.is_some())
}

/// Serializes the read-modify-write of a conversation file between every writer (P78, P87): this
/// process's mutex, plus an OS file lock on `<dir>/.writes.lock` for the other processes that
/// write the same directory (the CLI leaves `message_agent` notes in the desktop's). Held only
/// around file I/O, never a model call; the listing skips it, as it isn't `.json`.
static CONVERSATION_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct ConversationWriteGuard {
    _in_process: std::sync::MutexGuard<'static, ()>,
    _file: std::fs::File,
}

impl ConversationWriteGuard {
    fn acquire(dir: &Path) -> anyhow::Result<Self> {
        let in_process = CONVERSATION_WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        std::fs::create_dir_all(dir).with_context(|| format!("failed to create conversations directory at {}", dir.display()))?;
        let path = dir.join(".writes.lock");
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).with_context(|| format!("failed to open {}", path.display()))?;
        file.lock().with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(Self { _in_process: in_process, _file: file })
    }
}

/// The longest title `rename_conversation` keeps — a sidebar label, not a place for prose.
pub const MAX_CONVERSATION_TITLE_CHARS: usize = 120;

/// Renames a saved conversation (P78). The title is trimmed and cut to
/// `MAX_CONVERSATION_TITLE_CHARS`; an empty one is an error. `false` when there's no such
/// conversation. `updated_at` is left alone: renaming isn't activity, so it doesn't reorder the list.
pub fn rename_conversation(dir: &Path, id: &str, title: &str) -> anyhow::Result<bool> {
    let title = title.trim();
    anyhow::ensure!(!title.is_empty(), "a conversation title can't be empty");
    let _guard = ConversationWriteGuard::acquire(dir)?;
    let Some(mut conversation) = load_conversation(dir, id)? else {
        return Ok(false);
    };
    conversation.title = title.chars().take(MAX_CONVERSATION_TITLE_CHARS).collect();
    save_conversation(dir, &conversation)?;
    Ok(true)
}

/// Moves a saved conversation into project `project` (P103), or out of any with `None`. `false` when there is no such
/// conversation. This is the only thing that changes a conversation's project after it was created: `append_messages`
/// never does, so a turn in flight (which read the conversation before) can't undo a move, and a move can't be lost
/// to one — both re-read the file under the same guard. The turn that is running finishes in the scope it began in.
/// Like a rename it isn't activity, so `updated_at` is left alone. Whether the project exists is the caller's to
/// check: it is a folder of the person's vault, which this directory knows nothing about.
pub fn set_conversation_project(dir: &Path, id: &str, project: Option<&str>) -> anyhow::Result<bool> {
    let _guard = ConversationWriteGuard::acquire(dir)?;
    let Some(mut conversation) = load_conversation(dir, id)? else {
        return Ok(false);
    };
    // P102: a working folder is fixed for the life of the conversation, and a conversation is in a folder or in a
    // project, not both. Taking it out of any project (`None`) is no change and stays allowed.
    if conversation.workdir.is_some() && project.is_some() {
        anyhow::bail!("a conversation that works in a folder can't be moved into a project");
    }
    if conversation.project_id.as_deref() != project {
        // The engine's session belongs to the project it worked in: elsewhere it would be a stranger's.
        conversation.engine_session_id = None;
    }
    conversation.project_id = project.map(str::to_string);
    save_conversation(dir, &conversation)?;
    Ok(true)
}

/// Deletes a saved conversation's file (P78), and the threads started from it (P125): a thread without its conversation would have no
/// message to hang from. `false` when there was none. Deleting a thread removes only its own file.
pub fn delete_conversation(dir: &Path, id: &str) -> anyhow::Result<bool> {
    let _guard = ConversationWriteGuard::acquire(dir)?;
    let path = dir.join(format!("{id}.json"));
    let deleted = match std::fs::remove_file(&path) {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => return Err(err).with_context(|| format!("failed to delete conversation file at {}", path.display())),
    };
    if deleted {
        // A folder that can't be read (locked, say) leaves the threads: the conversation is gone either way.
        for thread in list_conversations(dir).unwrap_or_default().into_iter().filter(|c| c.parent.as_ref().is_some_and(|p| p.conversation_id == id)) {
            let thread_path = dir.join(format!("{}.json", thread.id));
            if let Err(err) = std::fs::remove_file(&thread_path) {
                if err.kind() != io::ErrorKind::NotFound {
                    return Err(err).with_context(|| format!("failed to delete thread file at {}", thread_path.display()));
                }
            }
        }
    }
    Ok(deleted)
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
            let mut config: FileConfig = toml::from_str(&contents).with_context(|| format!("failed to parse config file at {}", path.display()))?;
            migrate_legacy_fallbacks(&mut config);
            Ok(config)
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
/// Starts (or connects to) one configured MCP server and lists its tools — stdio, HTTP, or HTTP with
/// OAuth. Shared by `bootstrap` and a node lending its MCP servers to a hub (P93).
pub async fn connect_mcp_server(server: &McpServerConfig) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    let name = server.name();
    match server {
        McpServerConfig::Stdio { command, args, env, .. } => {
            let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            McpToolProvider::connect_stdio(name, command, args, &env).await?.tools().await
        }
        McpServerConfig::Http { url, oauth, .. } if *oauth => {
            warden_core::tool::mcp_oauth::connect_http_oauth(name, url, &oauth_credential_store_path(name)).await?.tools().await
        }
        McpServerConfig::Http { url, headers, .. } => {
            let headers: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            McpToolProvider::connect_http(name, url, &headers).await?.tools().await
        }
    }
}

/// Adds `name`'s tools to `base_tools`, renaming one to `{name}__{tool}` only when its name is taken.
/// A server that didn't connect is skipped with a note.
pub fn add_mcp_tools(base_tools: &mut Vec<Arc<dyn Tool>>, name: &str, tools: anyhow::Result<Vec<Arc<dyn Tool>>>) {
    match tools {
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
        Err(err) => eprintln!("note: MCP server '{name}' unavailable, skipping: {err:#}\n"),
    }
}

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
/// itself uses — extracted out (P61) so callers that need the vault outside of
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
        Provider::Node => {
            let node = provider
                .node
                .clone()
                .filter(|n| !n.trim().is_empty())
                .with_context(|| format!("provider '{}' (node) has no node configured — the device id of the node whose model it is", provider.id))?;
            Arc::new(node_model::NodeModelProvider::new(node, model))
        }
    })
}

/// Checks a provider's key without spending a conversation (P10): builds it the way a chat would and asks it for
/// its model list (`ModelProvider::check_key`). Never goes through an `Orchestrator`, so it books nothing in the
/// spend ledger and counts against no limit. A model lent by a node has no key here, and one that can't even be
/// built (no key, no address) is reported with what is missing, not as a network failure.
pub async fn test_provider(provider: &ProviderConfig) -> KeyCheck {
    if provider.kind == Provider::Node {
        return KeyCheck::Unsupported("A model lent by a node has no key here: check the node in the Nodes list.".to_string());
    }
    match build_model_provider(provider, None) {
        Ok(model) => model.check_key().await,
        Err(err) => KeyCheck::Rejected(format!("{err:#}")),
    }
}

/// The model a provider or combo id names (P90) — what every place that lets someone pick a
/// model builds from: a provider is `build_model_provider`, a combo a `FallbackProvider` over its
/// providers in order (P79). `model_override` (a `--model` flag) only applies to a provider. A
/// combo member that can't be built (no key, an id that's gone) is left out with a note; a combo
/// with one usable member is just that member.
pub fn build_model_for(config: &FileConfig, id: &str, model_override: Option<String>) -> anyhow::Result<Arc<dyn ModelProvider>> {
    // Every provider carries the id it was given, so the spend ledger can say which one answered (P10). A combo of
    // several is a `FallbackProvider`, which reports its first member and names the one that took over.
    if let Some(provider) = config.providers.iter().find(|p| p.id == id) {
        return Ok(Labeled::wrap(&provider.id, build_model_provider(provider, model_override)?));
    }
    let Some(combo) = config.combos.iter().find(|c| c.id == id) else {
        anyhow::bail!("model '{id}' is neither a configured provider nor a combo");
    };
    let mut chain = combo_chain(config, combo);
    match chain.len() {
        0 => anyhow::bail!("none of combo '{id}'s providers can be used ({})", combo.providers.join(", ")),
        1 => {
            let (member, provider) = chain.remove(0);
            Ok(Labeled::wrap(member, provider))
        }
        _ => Ok(Arc::new(FallbackProvider::new(chain))),
    }
}

/// A combo's providers that could be built, in order, without repeats.
fn combo_chain(config: &FileConfig, combo: &ComboConfig) -> Vec<(String, Arc<dyn ModelProvider>)> {
    let mut chain: Vec<(String, Arc<dyn ModelProvider>)> = Vec::new();
    for id in &combo.providers {
        if chain.iter().any(|(seen, _)| seen == id) {
            continue;
        }
        let Some(provider) = config.providers.iter().find(|p| &p.id == id) else {
            eprintln!("note: combo '{}' names '{id}', which isn't a configured provider — skipped\n", combo.id);
            continue;
        };
        match build_model_provider(provider, None) {
            Ok(model) => chain.push((id.clone(), model)),
            Err(err) => eprintln!("note: combo '{}' can't use '{id}' — skipped: {err:#}\n", combo.id),
        }
    }
    chain
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
fn delegate_targets(config: &FileConfig, orchestrator: &Orchestrator, caller: Option<&str>) -> Vec<NamedSubAgent> {
    let mut targets = Vec::new();
    // P120: a caller that is part of the organization delegates only to the agents below it; one that isn't reaches
    // every agent, as before the hierarchy existed.
    let scope: Option<Vec<String>> = caller.filter(|c| org::is_in_hierarchy(&config.agents, c)).map(|c| org::subordinates_of(&config.agents, c));
    // One copy of the config for every agent that may delegate itself (P123), made only when there is one.
    let snapshot = config.agents.iter().any(|a| a.owner.is_none() && a.can_delegate_to_agents).then(|| snapshot_config(config)).flatten();
    // P84: a member's agent is theirs alone — never a target of the owner's chief.
    for agent in config.agents.iter().filter(|a| a.owner.is_none()).filter(|a| scope.as_ref().is_none_or(|scope| scope.contains(&a.id))) {
        let target_orchestrator = match &agent.provider_id {
            Some(provider_id) => {
                match build_model_for(config, provider_id, None) {
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
        let modeled = target_orchestrator.with_agent(Some(agent.id.clone()));
        // P122: its own level too — and never above the caller's, which `orchestrator` already carries.
        let read_only: Vec<String> = SAFE_AGENT_TOOLS.iter().map(|t| t.to_string()).collect();
        let level = warden_core::autonomy::Autonomy::from_level(agent.autonomy).unwrap_or(warden_core::autonomy::Autonomy::AskFirst);
        let capped = |o: Orchestrator| o.with_autonomy(level, &read_only).with_approval_rules(&agent.approval_required, None);
        // What this agent would delegate from: its own model and limits, but every tool, so each of ITS targets is narrowed to its own.
        let nested_base = capped(modeled.clone());
        let target_orchestrator = capped(modeled.with_allowed_tools(agent.allowed_tools.as_deref()));
        // P123: an agent that may delegate builds its own `delegate_to_agent` when a background task of it starts, reaching only
        // whoever the hierarchy lets it reach. Built then, not now, so the tools of every agent aren't built up front.
        let delegation: Option<DelegationSpawner> = match (&snapshot, agent.can_delegate_to_agents) {
            (Some(snapshot), true) => {
                let (snapshot, id) = (snapshot.clone(), agent.id.clone());
                Some(Arc::new(move || build_delegate_to_agent_tool(&snapshot, &nested_base, Some(&id))))
            }
            _ => None,
        };
        let persona = (!agent.persona.trim().is_empty()).then(|| agent.persona.clone());
        targets.push(NamedSubAgent {
            id: agent.id.clone(),
            description: agent.persona.clone(),
            orchestrator: target_orchestrator,
            persona,
            delegation,
        });
    }
    targets
}

pub fn build_delegate_to_agent_tool(config: &FileConfig, orchestrator: &Orchestrator, caller: Option<&str>) -> Option<Arc<dyn Tool>> {
    let targets = delegate_targets(config, orchestrator, caller);
    (!targets.is_empty()).then(|| {
        let tool = DelegateToAgentTool::new(targets);
        // The caller's own limit on the models it may pick (P123), or everyone's choices.
        Arc::new(match model_choices_of(config, caller) {
            Some(models) => tool.with_models(models),
            None => tool,
        }) as Arc<dyn Tool>
    })
}

/// Same tool as `build_delegate_to_agent_tool`, but its target list follows the agents on disk
/// during the turn: when `revision` moves (a `ManageAgentsTool` given the same `AgentsRevision`
/// created, edited or deleted an agent), the targets are rebuilt from `config_path` — so an agent
/// the chief just created can be delegated to in that same turn instead of from the next one. The
/// rebuilt targets clone the same `orchestrator` the first ones did, so they carry exactly the
/// same tools and delegation depth. An unreadable config keeps the previous list. `caller` is the agent delegating
/// (P120): in the organization it reaches only the agents below it. A caller with nobody below it yet gets no tool, so
/// the subordinate it creates in the same turn is reachable from the next one.
pub fn build_live_delegate_to_agent_tool(
    config_path: &Path,
    config: &FileConfig,
    orchestrator: &Orchestrator,
    revision: AgentsRevision,
    caller: Option<&str>,
) -> Option<Arc<dyn Tool>> {
    let targets = delegate_targets(config, orchestrator, caller);
    if targets.is_empty() {
        return None;
    }
    let path = config_path.to_path_buf();
    let base = orchestrator.clone();
    let models = model_choices_of(config, caller);
    let caller = caller.map(str::to_string);
    let resolver: AgentResolver = Arc::new(move || {
        let config = load_config_from_path(&path, false).ok()?;
        Some(delegate_targets(&config, &base, caller.as_deref()))
    });
    let tool = DelegateToAgentTool::live(targets, revision, resolver);
    Some(Arc::new(match models {
        Some(models) => tool.with_models(models),
        None => tool,
    }))
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
        return build_model_for(config, &active_id, overrides.model.clone());
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
        Provider::Node => ("node", None),
    };
    let env_var = match kind {
        Provider::Gemini => Some("GEMINI_API_KEY"),
        Provider::Openai => Some("OPENAI_API_KEY"),
        Provider::Anthropic | Provider::OpenaiCompatible | Provider::Node => None,
    };
    let api_key = resolve_secret(env_var.and_then(|v| std::env::var(v).ok()), api_key_from_config);

    let synthesized = ProviderConfig { id: id.to_string(), kind, api_key, base_url: None, model: config.model.clone(), node: None };
    Ok(Labeled::wrap(id, build_model_provider(&synthesized, overrides.model.clone())?))
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

    // Read here, before `config` is picked apart below (the Tavily key is moved out of it).
    let spend_guard = spend::build_spend_guard(&config);
    let models = model_choices(&config);

    let mut base_tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ReadFileTool::new(vault.clone())),
        Arc::new(WriteFileTool::new(vault.clone())),
        Arc::new(GenerateDocumentTool::new(generated_path.clone())),
        Arc::new(UsageStatsTool::new(default_conversations_dir())),
        Arc::new(history::SearchHistoryTool::new(default_conversations_dir())),
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
        let tools = connect_mcp_server(server).await;
        add_mcp_tools(&mut base_tools, server.name(), tools);
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
    let mut orchestrator =
        build_delegating_orchestrator(model_provider, vault, &base_tools, delegate_max_depth, generated_path, models)
            .with_delegation_limit(max_delegated_calls)
            .with_parallel_jobs(max_parallel_jobs as usize);
    if let Some(guard) = spend_guard {
        orchestrator = orchestrator.with_spend_guard(guard);
    }
    // P123: the background tasks of every turn are written down, so a screen can show them after the turn is over.
    if let Some(path) = resolve_agent_tasks_path(std::env::var("WARDEN_AGENT_TASKS").ok()) {
        orchestrator = orchestrator.with_task_recorder(Arc::new(agent_tasks::FileTaskRecorder::new(path)));
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
    models: Option<ModelChoices>,
) -> Orchestrator {
    let mut orchestrator = Orchestrator::new(model.clone(), vault.clone()).with_media_root(media_root.clone());
    for tool in base_tools {
        orchestrator.register_tool(tool.clone());
    }
    if depth > 0 {
        let sub = build_delegating_orchestrator(model, vault, base_tools, depth - 1, media_root, models.clone());
        let delegate = DelegateTool::new(sub);
        orchestrator.register_tool(Arc::new(match models {
            Some(models) => delegate.with_models(models),
            None => delegate,
        }));
    }
    orchestrator
}

/// The models an agent may choose between for each task it delegates (P123): every configured provider and combo, by id.
/// `None` when there is nothing to choose between (fewer than two), so no `model` argument is offered.
pub fn model_choices(config: &FileConfig) -> Option<ModelChoices> {
    all_model_choices(config).filter(|choices| choices.ids.len() >= 2)
}

/// `model_choices` for one agent (P123): when a person limited the models it may pick (`AgentConfig::delegation_models`), only those
/// that still exist are offered, in the order given, and the first is what a delegation naming none gets — even a list of one, which is the
/// person dictating the model. A limit with nothing left in it offers no choice at all (the sub-agent's own model runs), never everything.
pub fn model_choices_for(config: &FileConfig, agent: Option<&AgentConfig>) -> Option<ModelChoices> {
    let Some(agent) = agent.filter(|a| !a.delegation_models.is_empty()) else {
        return model_choices(config);
    };
    let mut choices = all_model_choices(config)?;
    let allowed: Vec<String> = agent.delegation_models.iter().filter(|id| choices.ids.contains(id)).cloned().collect();
    if allowed.is_empty() {
        eprintln!("note: agent '{}' may only delegate with {}, which no longer exist — it gets no choice of model\n", agent.id, agent.delegation_models.join(", "));
        return None;
    }
    choices.hints.retain(|(id, _)| allowed.contains(id));
    choices.default = allowed.first().cloned();
    choices.ids = allowed;
    Some(choices)
}

/// The models of `caller` (an agent id) for its delegations: its own limit, or everyone's choices.
fn model_choices_of(config: &FileConfig, caller: Option<&str>) -> Option<ModelChoices> {
    model_choices_for(config, caller.and_then(|id| config.agents.iter().find(|a| a.id == id)))
}

fn all_model_choices(config: &FileConfig) -> Option<ModelChoices> {
    let mut ids: Vec<String> = config.providers.iter().map(|p| p.id.clone()).chain(config.combos.iter().map(|c| c.id.clone())).collect();
    // The named policies (P123) come after the plain ids; one that clashes with an id or points at nothing is left out.
    let mut policies: Vec<&ModelPolicyConfig> = Vec::new();
    for policy in &config.model_policies {
        let clashes = ids.contains(&policy.id) || policies.iter().any(|p| p.id == policy.id);
        if clashes || !ids.contains(&policy.model) {
            eprintln!("note: model policy '{}' clashes with another model or names '{}', which isn't a provider or combo — skipped\n", policy.id, policy.model);
            continue;
        }
        policies.push(policy);
    }
    ids.extend(policies.iter().map(|p| p.id.clone()));
    if ids.is_empty() {
        return None;
    }
    let hints = policies.iter().map(|p| (p.id.clone(), if p.description.is_empty() { format!("the same as '{}'", p.model) } else { p.description.clone() })).collect();
    let routes: Vec<(String, String)> = policies.iter().map(|p| (p.id.clone(), p.model.clone())).collect();
    let snapshot = snapshot_config(config)?;
    Some(ModelChoices {
        ids,
        hints,
        default: None,
        resolve: Arc::new(move |id| {
            let target = routes.iter().find(|(policy, _)| policy == id).map_or(id, |(_, model)| model.as_str());
            build_model_for(&snapshot, target, None)
        }),
    })
}

/// A copy of `config` a closure can keep. `FileConfig` isn't `Clone`; a round trip through its own file format is how this crate copies one.
fn snapshot_config(config: &FileConfig) -> Option<Arc<FileConfig>> {
    let copy: FileConfig = toml::from_str(&toml::to_string(config).ok()?).ok()?;
    Some(Arc::new(copy))
}

/// Where the log of delegated tasks is written (P123): `WARDEN_AGENT_TASKS` (a file path) wins over the default location,
/// and `off` writes none.
pub fn resolve_agent_tasks_path(from_env: Option<String>) -> Option<PathBuf> {
    match from_env.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        Some(value) if value.eq_ignore_ascii_case("off") => None,
        Some(value) => Some(PathBuf::from(value)),
        None => agent_tasks::default_agent_tasks_path(),
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
    fn a_config_from_before_sessao_105_still_loads_and_a_save_drops_the_storage_keys() {
        let dir = std::env::temp_dir().join(format!(
            "warden-legacy-storage-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "storage_provider = \"remote_node\"\n# mine\nvault_path = \"/v\"\n\n[remote_node]\nserver_url = \"ws://h:7420\"\ndevice_id = \"a\"\n",
        )
        .unwrap();

        let config = load_config_from_path(&path, true).unwrap();
        assert_eq!(config.vault_path.as_deref(), Some("/v"));
        save_config(&path, &config).unwrap();

        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("storage_provider") && !saved.contains("remote_node"), "{saved}");
        assert!(saved.contains("# mine") && saved.contains("vault_path"), "{saved}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_removed_agent_closes_the_nodes_it_was_alone_on() {
        let mut nodes = vec![
            NodeAccessConfig { id: "a".into(), enabled: true, agents: vec!["ops".into()], require_approval: false },
            NodeAccessConfig { id: "b".into(), enabled: true, agents: vec!["ops".into(), "dev".into()], require_approval: false },
            NodeAccessConfig { id: "c".into(), enabled: true, agents: vec![], require_approval: false },
        ];
        forget_agent_in_nodes(&mut nodes, "ops");
        assert_eq!((nodes[0].enabled, nodes[0].agents.len()), (false, 0), "never falls through to everyone");
        assert_eq!((nodes[1].enabled, nodes[1].agents.clone()), (true, vec!["dev".to_string()]));
        assert!(nodes[2].enabled && nodes[2].agents.is_empty());
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
                node: None,
            }],
            active_provider: Some("ollama-local".to_string()),
            model_policies: vec![ModelPolicyConfig { id: "cheap".to_string(), model: "ollama-local".to_string(), description: "simple work".to_string() }],
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
                can_message_agents: true,
                can_manage_tasks: false,
                allowed_tools: Some(vec!["read_file".to_string(), "use_skill".to_string()]),
                autonomy: 2,
                approval_required: vec![warden_core::autonomy::Category::CriticalInfra, warden_core::autonomy::Category::DeleteData],
                role: Some("Head of the crew".to_string()),
                reports_to: None,
                owner: None,
                shared_with: Vec::new(),
                delegation_models: Vec::new(),
            }],
            tool_categories: vec![risk::ToolCategoryConfig { tool: "pay".to_string(), category: warden_core::autonomy::Category::SpendMoney }],
            combos: vec![ComboConfig { id: "local-first".to_string(), providers: vec!["ollama-local".to_string()] }],
            legacy_fallback_providers: Vec::new(),
            legacy_storage_provider: None,
            legacy_remote_node: None,
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
            tasks: vec![TaskConfig {
                id: "morning".to_string(),
                agent: Some("helper".to_string()),
                prompt: "summarize".to_string(),
                every: None,
                cron: Some("0 8 * * 1-5".to_string()),
                once: None,
                timezone: Some("America/Sao_Paulo".to_string()),
                enabled: false,
            }],
            webhooks: vec![webhooks::WebhookConfig { id: "build".to_string(), agent: Some("helper".to_string()), prompt: "why did it fail?".to_string(), enabled: false, auth: webhooks::WebhookAuth::Hmac }],
            outreach: vec![OutreachConfig { agent: "helper".to_string(), forward: vec!["telegram".to_string()] }],
            nodes: vec![NodeAccessConfig { id: "home-pc".to_string(), enabled: true, agents: vec!["helper".to_string()], require_approval: true }],
            users: Vec::new(),
            removed_users: Vec::new(),
            recovery_policy: recovery::RecoveryPolicy::Private,
            recovery_public_key: None,
            spaces: Vec::new(),
            truthid_network: Default::default(),
            truthid_rpc_url: None,
            truthid_public_url: None,
            learning: Default::default(),
            telegram: bot_access::TelegramSettings { allowed_users: vec![42], pairing: true, members: [("42".to_string(), "ana".to_string())].into() },
            whatsapp: bot_access::WhatsAppSettings { allowed_chats: vec!["5511999999999".to_string()], pairing: false, members: Default::default() },
            bot_hub: Some(bot_access::BotHubSettings { url: "ws://192.168.0.5:7420".to_string() }),
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
                tools_used: Vec::new(),
            }],
            created_at: updated_at,
            updated_at,
            agent_id: None,
            provider_id: None,
            project_id: None,
            engine_session_id: None,
            workdir: None,
            parent: None,
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

    /// P84 fatia 4: a member's conversations are encrypted on disk, and locked without the key.
    #[test]
    fn conversations_of_an_encrypted_folder_are_sealed_and_locked_without_the_key() {
        let dir = temp_dir("sealed");
        let key = member_crypto::new_key();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(member_crypto::MARKER), b"1").unwrap();
        member_crypto::unlock(&[&dir], &key);

        let conversation = sample_conversation("c1", 100);
        save_conversation(&dir, &conversation).unwrap();
        let on_disk = std::fs::read(dir.join("c1.json")).unwrap();
        assert!(warden_core::memory::VaultCipher::is_sealed(&on_disk));
        assert!(!String::from_utf8_lossy(&on_disk).contains(&conversation.title), "the title is on disk");
        assert_eq!(load_conversation(&dir, "c1").unwrap(), Some(conversation.clone()));
        assert_eq!(list_conversations(&dir).unwrap(), vec![conversation.clone()]);
        assert!(rename_conversation(&dir, "c1", "Novo").unwrap());
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().title, "Novo");

        member_crypto::lock(&[&dir]);
        assert!(load_conversation(&dir, "c1").is_err());
        assert!(list_conversations(&dir).is_err());
        assert!(save_conversation(&dir, &conversation).is_err(), "nothing readable is written while locked");
        assert_eq!(std::fs::read(dir.join("c1.json")).unwrap()[..4], *b"WRD1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn append_messages_keeps_what_another_writer_added() {
        let dir = temp_dir("append");
        let note = |id: &str, role: ChatRole| ConversationMessage {
            id: id.into(),
            role,
            content: id.into(),
            created_at: 1,
            usage: None,
            attachments: Vec::new(),
            generated_files: Vec::new(),
            tools_used: Vec::new(),
        };
        let options = AppendOptions { title_seed: "first words", agent_id: Some("writer"), provider_id: Some(Some("openai")), project_id: None, create: true, ..Default::default() };
        let created = append_messages(&dir, "c1", options, vec![note("hi", ChatRole::User)]).unwrap().unwrap();
        assert_eq!((created.title.as_str(), created.agent_id.as_deref(), created.provider_id.as_deref()), ("first words", Some("writer"), Some("openai")));

        // Another writer (an agent answering a note) adds to it; the screen's next append keeps it.
        append_to_conversation(&dir, "c1", "", Some("writer"), None, false, vec![note("from B", ChatRole::Assistant)]).unwrap();
        let options = AppendOptions { agent_id: Some("writer"), ..Default::default() };
        let saved = append_messages(&dir, "c1", options, vec![note("me again", ChatRole::User)]).unwrap().unwrap();
        let contents: Vec<&str> = saved.messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, ["hi", "from B", "me again"]);
        assert_eq!(saved.provider_id.as_deref(), Some("openai"), "left alone when not given");
        assert_eq!(load_conversation(&dir, "c1").unwrap(), Some(saved));

        assert_eq!(append_messages(&dir, "gone", AppendOptions::default(), vec![note("x", ChatRole::User)]).unwrap(), None);
        assert_eq!(list_conversations(&dir).unwrap().len(), 1, "the lock file isn't a conversation");
        std::fs::remove_dir_all(&dir).ok();
    }

    const CHILD_CONVERSATIONS: &str = "WARDEN_CONVERSATION_LOCK_CHILD";

    /// Run by `conversation_writes_wait_for_another_process` in a child process. Does nothing in a
    /// normal test run.
    #[test]
    fn child_holds_the_conversations_lock() {
        let Ok(dir) = std::env::var(CHILD_CONVERSATIONS) else { return };
        let dir = PathBuf::from(dir);
        let _held = ConversationWriteGuard::acquire(&dir).unwrap();
        std::fs::write(dir.join("held"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
    }

    #[test]
    fn conversation_writes_wait_for_another_process() {
        let dir = temp_dir("cross-process");
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::child_holds_the_conversations_lock", "--nocapture"])
            .env(CHILD_CONVERSATIONS, &dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !dir.join("held").exists() {
            assert!(std::time::Instant::now() < deadline, "the child never took the lock");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let started = std::time::Instant::now();
        save_conversation(&dir, &sample_conversation("c1", 1)).unwrap();
        assert!(rename_conversation(&dir, "c1", "renamed").unwrap());
        assert!(started.elapsed() >= std::time::Duration::from_millis(300), "{:?}", started.elapsed());
        assert!(child.wait().unwrap().success());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Calls the `ping` tool on the first request and answers once it has the result.
    struct PingsThenAnswers;

    #[async_trait::async_trait]
    impl ModelProvider for PingsThenAnswers {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<warden_core::tool::ToolSpec>) -> anyhow::Result<warden_core::model::ChatStream> {
            let called = messages.iter().any(|m| m.role == warden_core::model::Role::Tool);
            let tool_calls = if called {
                Vec::new()
            } else {
                vec![warden_core::model::ToolCall { id: "c1".into(), name: "ping".into(), arguments: serde_json::json!({}), thought_signature: None }]
            };
            Ok(warden_core::model::response_stream(warden_core::model::Response { content: if called { "pong".into() } else { String::new() }, tool_calls, usage: None }))
        }
    }

    struct PingTool;

    #[async_trait::async_trait]
    impl warden_core::tool::Tool for PingTool {
        fn spec(&self) -> warden_core::tool::ToolSpec {
            warden_core::tool::ToolSpec { name: "ping".into(), description: "pings".into(), parameters: serde_json::json!({}) }
        }
        async fn call(&self, _args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!("pong"))
        }
    }

    #[tokio::test]
    async fn handle_turn_saves_which_tools_the_turn_ran_on_the_assistants_message() {
        let root = temp_dir("turn-tools");
        let dir = root.join("conversations");
        let mut orchestrator = Orchestrator::new(Arc::new(PingsThenAnswers), Arc::new(Vault::new(root.join("vault"))));
        orchestrator.register_tool(Arc::new(PingTool));

        handle_turn(&orchestrator, &dir, "c1", "hi", "ping it", Vec::new()).await.unwrap();

        let saved = load_conversation(&dir, "c1").unwrap().unwrap();
        assert!(saved.messages[0].tools_used.is_empty(), "the person's message has none");
        assert_eq!(saved.messages[1].tools_used, vec!["ping".to_string()]);
        let raw = std::fs::read_to_string(dir.join("c1.json")).unwrap();
        assert!(raw.contains("\"toolsUsed\":[\"ping\"]") || raw.contains("\"toolsUsed\": [\n"), "{raw}");
        std::fs::remove_dir_all(&root).ok();
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

    /// Answers with every message it was sent ("|"-separated), so a test sees exactly what a turn told the model.
    struct EchoesWhatItWasTold;

    #[async_trait::async_trait]
    impl ModelProvider for EchoesWhatItWasTold {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<warden_core::tool::ToolSpec>) -> anyhow::Result<warden_core::model::ChatStream> {
            let told = messages.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("|");
            Ok(warden_core::model::response_stream(warden_core::model::Response { content: told, tool_calls: Vec::new(), usage: None }))
        }
    }

    /// How a project's briefing begins (`Project::briefing`): what tells a project turn from an ordinary one.
    const PROJECT_BRIEFING: &str = "You are working in the project \"Tax return\".";

    /// A hub-like setup: a vault with the project `tax` (instructions, one file) and a note outside it.
    fn project_setup(name: &str) -> (PathBuf, Orchestrator) {
        let root = temp_dir(name);
        let vault = Arc::new(Vault::new(root.join("vault")));
        warden_core::project::ProjectStore::new(vault.clone())
            .save(&warden_core::project::Project { id: "tax".into(), name: "Tax return".into(), description: String::new(), instructions: "Answer in Portuguese.".into(), workdir: None, code: false })
            .unwrap();
        vault.write("projects/tax/jan.md", "january receipts").unwrap();
        vault.write("diary.md", "the diary").unwrap();
        let mut orchestrator = Orchestrator::new(Arc::new(EchoesWhatItWasTold), vault.clone());
        orchestrator.register_tool(Arc::new(ReadFileTool::new(vault)));
        (root, orchestrator)
    }

    /// P103: a conversation created in a project keeps it, runs in the project's folder with its instructions, and
    /// no later turn — from any client, whatever it sends — takes it out.
    #[tokio::test]
    async fn a_conversation_created_in_a_project_runs_there_and_keeps_it_whatever_is_sent_later() {
        let (root, orchestrator) = project_setup("project-turn");
        let dir = root.join("conversations");

        let first = handle_agent_turn(&orchestrator, &dir, "c1", "hi", "january", Vec::new(), None, Some("tax"), None).await.unwrap();
        assert!(first.content.starts_with(PROJECT_BRIEFING), "the first thing the model is told is the project's briefing: {}", first.content);
        assert!(first.content.contains("Answer in Portuguese.") && first.content.contains("jan.md"), "with its instructions and files: {}", first.content);
        assert!(!first.content.contains("the diary"), "and nothing of the rest of the vault: {}", first.content);
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().project_id.as_deref(), Some("tax"));

        // A client that doesn't know projects, and one naming another project, both leave it where it is.
        let second = handle_agent_turn(&orchestrator, &dir, "c1", "", "again", Vec::new(), None, None, None).await.unwrap();
        assert!(second.content.starts_with(PROJECT_BRIEFING), "still in the project: {}", second.content);
        handle_agent_turn(&orchestrator, &dir, "c1", "", "once more", Vec::new(), None, Some("other"), None).await.unwrap();
        let saved = load_conversation(&dir, "c1").unwrap().unwrap();
        assert_eq!((saved.project_id.as_deref(), saved.messages.len()), (Some("tax"), 6));

        // Outside a project the same orchestrator still sees the whole vault, and is told no briefing.
        let plain = handle_agent_turn(&orchestrator, &dir, "c2", "hi", "diary", Vec::new(), None, None, None).await.unwrap();
        assert!(plain.content.contains("the diary") && !plain.content.starts_with(PROJECT_BRIEFING), "{}", plain.content);
        assert_eq!(load_conversation(&dir, "c2").unwrap().unwrap().project_id, None);
        std::fs::remove_dir_all(&root).ok();
    }

    /// A conversation `id` in `dir` with three turns ("alpha", "beta", "gamma question"), and the id of the message of the second one.
    async fn three_turns(orchestrator: &Orchestrator, dir: &Path, id: &str, project: Option<&str>) -> String {
        for (n, text) in ["alpha question", "beta question", "gamma question"].into_iter().enumerate() {
            handle_agent_turn(orchestrator, dir, id, "hi", text, Vec::new(), None, project.filter(|_| n == 0), None).await.unwrap();
        }
        load_conversation(dir, id).unwrap().unwrap().messages[2].id.clone()
    }

    /// P125: a thread's model sees the conversation up to the message it came from and then the thread, and never what came after.
    #[tokio::test]
    async fn a_threads_model_sees_the_conversation_up_to_its_message_and_then_the_thread() {
        let (root, orchestrator) = project_setup("thread-context");
        let dir = root.join("conversations");
        let anchor = three_turns(&orchestrator, &dir, "c1", None).await;
        let link = ThreadParent { conversation_id: "c1".into(), message_id: anchor };

        let first = handle_agent_turn_in(&orchestrator, &dir, "t1", "", "inside the thread", Vec::new(), None, None, None, Some(&link)).await.unwrap();
        assert!(first.content.contains("alpha question") && first.content.contains("beta question"), "what came before the message: {}", first.content);
        assert!(first.content.contains("inside the thread"), "{}", first.content);
        assert!(!first.content.contains("gamma question"), "nothing after the message it came from: {}", first.content);
        let saved = load_conversation(&dir, "t1").unwrap().unwrap();
        assert_eq!((saved.parent.as_ref(), saved.messages.len()), (Some(&link), 2), "the thread holds only its own messages");

        // The link is the file's, so a later turn needs none, and what it sends then is ignored.
        let second = handle_agent_turn_in(&orchestrator, &dir, "t1", "", "once more", Vec::new(), None, None, None, None).await.unwrap();
        assert!(second.content.contains("beta question") && second.content.contains("inside the thread") && !second.content.contains("gamma question"), "{}", second.content);
        let other = ThreadParent { conversation_id: "c1".into(), message_id: load_conversation(&dir, "c1").unwrap().unwrap().messages[0].id.clone() };
        handle_agent_turn_in(&orchestrator, &dir, "t1", "", "and again", Vec::new(), None, None, None, Some(&other)).await.unwrap();
        assert_eq!(load_conversation(&dir, "t1").unwrap().unwrap().parent, Some(link), "a thread keeps the message it came from");

        // The conversation it came from was not touched.
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().messages.len(), 6);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_thread_starts_in_the_project_of_the_conversation_it_came_from() {
        let (root, orchestrator) = project_setup("thread-project");
        let dir = root.join("conversations");
        let anchor = three_turns(&orchestrator, &dir, "c1", Some("tax")).await;
        let link = ThreadParent { conversation_id: "c1".into(), message_id: anchor };

        // Whatever the thread is told to start in, it starts where its conversation is.
        let reply = handle_agent_turn_in(&orchestrator, &dir, "t1", "", "in the thread", Vec::new(), None, Some("other"), None, Some(&link)).await.unwrap();
        assert!(reply.content.contains(PROJECT_BRIEFING), "{}", reply.content);
        assert_eq!(load_conversation(&dir, "t1").unwrap().unwrap().project_id.as_deref(), Some("tax"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_thread_needs_a_message_of_a_conversation_that_is_not_itself_a_thread() {
        let (root, orchestrator) = project_setup("thread-refusals");
        let dir = root.join("conversations");
        let anchor = three_turns(&orchestrator, &dir, "c1", None).await;
        let link = |conversation: &str, message: &str| ThreadParent { conversation_id: conversation.into(), message_id: message.into() };
        handle_agent_turn_in(&orchestrator, &dir, "t1", "", "first", Vec::new(), None, None, None, Some(&link("c1", &anchor))).await.unwrap();
        let in_thread = load_conversation(&dir, "t1").unwrap().unwrap().messages[0].id.clone();

        for (id, from, why) in [
            ("t2", link("ghost", &anchor), "no such conversation"),
            ("t3", link("c1", "no-such-message"), "no such message"),
            ("t4", link("t1", &in_thread), "a thread of a thread"),
            ("c1b", link("c1b", &anchor), "a thread of itself"),
            ("t5", link("../escape", &anchor), "a path instead of a conversation id"),
            ("t6", link("c1/../c1", &anchor), "a path that ends at a real conversation"),
        ] {
            assert!(handle_agent_turn_in(&orchestrator, &dir, id, "", "x", Vec::new(), None, None, None, Some(&from)).await.is_err(), "{why}");
            assert!(load_conversation(&dir, id).unwrap().is_none(), "nothing was saved: {why}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// A conversation has no limit of its own, but a thread resends at most `THREAD_CONTEXT_MAX` messages of it.
    #[tokio::test]
    async fn a_thread_resends_at_most_the_last_messages_that_end_at_its_message() {
        let (root, orchestrator) = project_setup("thread-cap");
        let dir = root.join("conversations");
        let message = |i: usize| ConversationMessage {
            id: format!("m{i}"),
            role: if i.is_multiple_of(2) { ChatRole::User } else { ChatRole::Assistant },
            content: format!("line-{i:03}-end"),
            created_at: i as i64,
            usage: None,
            attachments: Vec::new(),
            generated_files: Vec::new(),
            tools_used: Vec::new(),
        };
        let long = Conversation { id: "long".into(), title: "long".into(), messages: (0..100).map(message).collect(), created_at: 0, updated_at: 0, agent_id: None, provider_id: None, project_id: None, engine_session_id: None, workdir: None, parent: None };
        save_conversation(&dir, &long).unwrap();
        let link = ThreadParent { conversation_id: "long".into(), message_id: "m89".into() };

        let reply = handle_agent_turn_in(&orchestrator, &dir, "t1", "", "in the thread", Vec::new(), None, None, None, Some(&link)).await.unwrap();
        let first_kept = 90 - THREAD_CONTEXT_MAX;
        assert!(reply.content.contains(&format!("line-{first_kept:03}-end")) && reply.content.contains("line-089-end"), "{}", reply.content);
        assert!(!reply.content.contains(&format!("line-{:03}-end", first_kept - 1)) && !reply.content.contains("line-090-end"), "{}", reply.content);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn deleting_a_conversation_deletes_its_threads_and_deleting_a_thread_leaves_it() {
        let (root, orchestrator) = project_setup("thread-delete");
        let dir = root.join("conversations");
        let anchor = three_turns(&orchestrator, &dir, "c1", None).await;
        three_turns(&orchestrator, &dir, "c2", None).await;
        let link = ThreadParent { conversation_id: "c1".into(), message_id: anchor };
        handle_agent_turn_in(&orchestrator, &dir, "t1", "", "in c1", Vec::new(), None, None, None, Some(&link)).await.unwrap();
        let other = ThreadParent { conversation_id: "c2".into(), message_id: load_conversation(&dir, "c2").unwrap().unwrap().messages[2].id.clone() };
        handle_agent_turn_in(&orchestrator, &dir, "t2", "", "in c2", Vec::new(), None, None, None, Some(&other)).await.unwrap();

        assert!(delete_conversation(&dir, "t2").unwrap());
        assert!(load_conversation(&dir, "c2").unwrap().is_some(), "the conversation stays when its thread goes");

        assert!(delete_conversation(&dir, "c1").unwrap());
        assert!(load_conversation(&dir, "t1").unwrap().is_none(), "its thread went with it");
        assert!(load_conversation(&dir, "c2").unwrap().is_some(), "another conversation is untouched");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_conversation_saved_before_threads_loads_and_a_thread_keeps_its_link_on_disk() {
        let old = r#"{"id":"c","title":"t","messages":[],"createdAt":1,"updatedAt":1}"#;
        assert_eq!(serde_json::from_str::<Conversation>(old).unwrap().parent, None);
        let dir = temp_dir("thread-disk");
        let link = ThreadParent { conversation_id: "c".into(), message_id: "m".into() };
        append_messages(&dir, "t", AppendOptions { title_seed: "x", thread_of: Some(&link), create: true, ..Default::default() }, Vec::new()).unwrap();
        assert!(std::fs::read_to_string(dir.join("t.json")).unwrap().contains(r#""parent""#));
        assert_eq!(load_conversation(&dir, "t").unwrap().unwrap().parent, Some(link));
        assert!(!std::fs::read_to_string({ append_messages(&dir, "plain", AppendOptions { title_seed: "x", create: true, ..Default::default() }, Vec::new()).unwrap(); dir.join("plain.json") }).unwrap().contains("parent"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_conversation_cannot_start_in_a_project_that_does_not_exist() {
        let (root, orchestrator) = project_setup("project-unknown");
        let dir = root.join("conversations");
        for id in ["nope", "loose"] {
            let err = handle_agent_turn(&orchestrator, &dir, "c1", "hi", "hi", Vec::new(), None, Some(id), None).await.unwrap_err();
            assert!(err.to_string().contains("there is no project"), "{id}: {err:#}");
        }
        assert!(handle_agent_turn(&orchestrator, &dir, "c1", "hi", "hi", Vec::new(), None, Some("../x"), None).await.is_err());
        assert_eq!(load_conversation(&dir, "c1").unwrap(), None, "nothing was run or saved");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Removing a project only removes `PROJECT.md`: what was in it goes on as ordinary conversations.
    /// P102: a conversation in no project starts in a folder, keeps it whatever is sent later, is told about it, and
    /// still has the person's own vault; a project wins over a folder; a folder that is gone is an error, not a fall
    /// back to the vault.
    #[tokio::test]
    async fn a_conversation_created_in_a_folder_keeps_it_and_is_told_about_it() {
        let (root, orchestrator) = project_setup("folder-turn");
        let dir = root.join("conversations");
        let folder = root.join("work");
        let other = root.join("other");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let (folder, other) = (folder.to_str().unwrap(), other.to_str().unwrap());

        let first = handle_agent_turn(&orchestrator, &dir, "c1", "hi", "plan", Vec::new(), None, None, Some(folder)).await.unwrap();
        assert!(first.content.contains(&format!("works in the folder '{folder}'")), "the model is told the folder: {}", first.content);
        assert!(first.content.contains("the diary"), "the person's own vault is still in its context: {}", first.content);
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().workdir.as_deref(), Some(folder));

        // A later turn that names another folder (or none) doesn't move it.
        let second = handle_agent_turn(&orchestrator, &dir, "c1", "", "again", Vec::new(), None, None, Some(other)).await.unwrap();
        assert!(second.content.contains(&format!("works in the folder '{folder}'")), "{}", second.content);
        handle_agent_turn(&orchestrator, &dir, "c1", "", "once more", Vec::new(), None, None, None).await.unwrap();
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().workdir.as_deref(), Some(folder));

        // A conversation in a project has no folder: the project wins.
        let in_project = handle_agent_turn(&orchestrator, &dir, "c2", "hi", "jan", Vec::new(), None, Some("tax"), Some(folder)).await.unwrap();
        assert!(in_project.content.starts_with(PROJECT_BRIEFING) && !in_project.content.contains("works in the folder"), "{}", in_project.content);
        assert_eq!(load_conversation(&dir, "c2").unwrap().unwrap().workdir, None);

        // The folder is gone: an error, nothing run and nothing saved for a new conversation.
        let gone = root.join("gone");
        let err = handle_agent_turn(&orchestrator, &dir, "c3", "hi", "x", Vec::new(), None, None, Some(gone.to_str().unwrap())).await.unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
        assert!(load_conversation(&dir, "c3").unwrap().is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    /// P102 fatia 2: a folder on a node is set up by the hub; a channel that didn't (no briefing) refuses the turn
    /// instead of running it as if there were no folder, and nothing is saved for it.
    #[tokio::test]
    async fn a_folder_on_a_node_is_only_run_by_a_caller_that_set_it_up() {
        let (root, orchestrator) = project_setup("node-folder");
        let dir = root.join("conversations");
        let err = handle_agent_turn(&orchestrator, &dir, "c1", "hi", "plan", Vec::new(), None, None, Some("node:node-a-1:proj")).await.unwrap_err();
        assert!(err.to_string().contains("only the hub can set up"), "{err}");
        assert!(load_conversation(&dir, "c1").unwrap().is_none());

        let set_up = orchestrator.with_briefing("This conversation works on a node.".to_string());
        let reply = handle_agent_turn(&set_up, &dir, "c1", "hi", "plan", Vec::new(), None, None, Some("node:node-a-1:proj")).await.unwrap();
        assert!(reply.content.starts_with("This conversation works on a node."), "{}", reply.content);
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().workdir.as_deref(), Some("node:node-a-1:proj"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_conversation_in_a_folder_cannot_be_moved_into_a_project_and_old_files_still_load() {
        let (root, orchestrator) = project_setup("folder-move");
        let dir = root.join("conversations");
        let folder = root.join("work");
        std::fs::create_dir_all(&folder).unwrap();
        handle_agent_turn(&orchestrator, &dir, "c1", "hi", "plan", Vec::new(), None, None, Some(folder.to_str().unwrap())).await.unwrap();

        assert!(set_conversation_project(&dir, "c1", Some("tax")).is_err());
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().project_id, None, "nothing changed");
        assert!(set_conversation_project(&dir, "c1", None).unwrap(), "out of any project is no change, and fine");

        let old: Conversation = serde_json::from_str(r#"{"id":"o","title":"t","messages":[],"createdAt":1,"updatedAt":1}"#).unwrap();
        assert_eq!(old.workdir, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("workdir"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_conversation_whose_project_was_removed_goes_on_as_an_ordinary_one() {
        let (root, orchestrator) = project_setup("project-removed");
        let dir = root.join("conversations");
        handle_agent_turn(&orchestrator, &dir, "c1", "hi", "january", Vec::new(), None, Some("tax"), None).await.unwrap();

        warden_core::project::ProjectStore::new(orchestrator.vault().clone()).delete("tax").unwrap();
        let after = handle_agent_turn(&orchestrator, &dir, "c1", "", "diary", Vec::new(), None, None, None).await.unwrap();
        assert!(after.content.contains("the diary") && !after.content.starts_with(PROJECT_BRIEFING), "the whole vault again, no briefing: {}", after.content);
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().messages.len(), 4, "the conversation is intact");
        std::fs::remove_dir_all(&root).ok();
    }

    /// `append_messages` is what the desktop calls around every turn: only a *new* conversation takes the project.
    #[test]
    fn a_conversation_can_be_moved_between_projects_and_out_of_one_without_counting_as_activity() {
        let dir = temp_dir("move-project");
        let note = |text: &str| ConversationMessage { id: message_id(), role: ChatRole::User, content: text.into(), created_at: 1, usage: None, attachments: Vec::new(), generated_files: Vec::new(), tools_used: Vec::new() };
        let new = AppendOptions { title_seed: "t", project_id: Some("tax"), create: true, ..Default::default() };
        let created = append_messages(&dir, "c1", new, vec![note("a")]).unwrap().unwrap();

        assert!(set_conversation_project(&dir, "c1", Some("garden")).unwrap());
        let moved = load_conversation(&dir, "c1").unwrap().unwrap();
        assert_eq!((moved.project_id.as_deref(), moved.updated_at, moved.messages.len()), (Some("garden"), created.updated_at, 1), "moved, nothing else touched");
        assert!(set_conversation_project(&dir, "c1", None).unwrap());
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().project_id, None);
        assert!(!set_conversation_project(&dir, "nope", Some("tax")).unwrap(), "no such conversation");
        assert!(load_conversation(&dir, "nope").unwrap().is_none(), "and none is made");

        // A later append — a turn that began before the move — doesn't undo it.
        set_conversation_project(&dir, "c1", Some("garden")).unwrap();
        append_messages(&dir, "c1", AppendOptions { project_id: Some("tax"), ..Default::default() }, vec![note("b")]).unwrap();
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().project_id.as_deref(), Some("garden"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn append_messages_gives_a_new_conversation_its_project_and_never_changes_an_existing_one() {
        let dir = temp_dir("append-project");
        let note = |text: &str| ConversationMessage { id: message_id(), role: ChatRole::User, content: text.into(), created_at: 1, usage: None, attachments: Vec::new(), generated_files: Vec::new(), tools_used: Vec::new() };
        let new = AppendOptions { title_seed: "t", project_id: Some("tax"), create: true, ..Default::default() };
        assert_eq!(append_messages(&dir, "c1", new, vec![note("a")]).unwrap().unwrap().project_id.as_deref(), Some("tax"));
        for sent in [None, Some("other")] {
            let again = AppendOptions { project_id: sent, ..Default::default() };
            assert_eq!(append_messages(&dir, "c1", again, vec![note("b")]).unwrap().unwrap().project_id.as_deref(), Some("tax"), "sent {sent:?}");
        }
        // A file from before projects reads without one, and a new file keeps no field when there is none.
        let plain = AppendOptions { title_seed: "t", create: true, ..Default::default() };
        append_messages(&dir, "c2", plain, vec![note("x")]).unwrap();
        assert!(!std::fs::read_to_string(dir.join("c2.json")).unwrap().contains("projectId"));
        std::fs::remove_dir_all(&dir).ok();
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

    /// `Arc<dyn ModelProvider>` isn't `Debug`, so `Result::unwrap_err` (which requires the `Ok`
    /// side to be `Debug` too) doesn't work directly on `resolve_model_provider`'s return type.
    fn expect_err<T>(result: anyhow::Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(err) => err.to_string(),
        }
    }

    fn provider_entry(id: &str, kind: Provider) -> ProviderConfig {
        ProviderConfig { id: id.to_string(), kind, api_key: Some("a-key".to_string()), base_url: None, model: Some("a-model".to_string()), node: None }
    }

    fn combo(id: &str, members: &[&str]) -> ComboConfig {
        ComboConfig { id: id.to_string(), providers: members.iter().map(|m| m.to_string()).collect() }
    }

    #[test]
    fn a_model_id_is_a_provider_or_a_combo_and_a_combo_keeps_what_can_be_built() {
        let mut no_key = provider_entry("no-key", Provider::Openai);
        no_key.api_key = None;
        let config = FileConfig {
            providers: vec![provider_entry("main", Provider::Gemini), provider_entry("spare", Provider::Anthropic), no_key],
            combos: vec![combo("fast", &["main", "no-key", "ghost", "spare", "spare"]), combo("lonely", &["no-key", "spare"]), combo("dead", &["no-key"])],
            ..Default::default()
        };
        let chain = combo_chain(&config, &config.combos[0]);
        assert_eq!(chain.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["main", "spare"]);

        assert_eq!(build_model_for(&config, "main", Some("override".into())).unwrap().model_id(), "override");
        assert_eq!(build_model_for(&config, "fast", None).unwrap().model_id(), "a-model");
        // One usable member: that provider itself, no wrapper needed.
        assert_eq!(build_model_for(&config, "lonely", None).unwrap().model_id(), "a-model");
        assert!(expect_err(build_model_for(&config, "dead", None)).contains("none of combo 'dead'"));
        assert!(expect_err(build_model_for(&config, "nope", None)).contains("neither"));
    }

    /// P10: whatever model is built reports the id the person gave it, so the spend ledger can say which provider
    /// answered: the provider's own id, a combo's first member (the others are named by the fallback that
    /// takes over), the one member that could be built, and the id synthesized for the old single-provider setup.
    #[test]
    fn a_built_model_reports_the_provider_id_the_ledger_keeps() {
        let mut no_key = provider_entry("no-key", Provider::Openai);
        no_key.api_key = None;
        let config = FileConfig {
            providers: vec![provider_entry("main", Provider::Gemini), provider_entry("spare", Provider::Anthropic), no_key],
            combos: vec![combo("fast", &["spare", "main"]), combo("lonely", &["no-key", "spare"])],
            ..Default::default()
        };
        assert_eq!(build_model_for(&config, "main", None).unwrap().provider_id(), "main");
        assert_eq!(build_model_for(&config, "fast", None).unwrap().provider_id(), "spare", "the first member answers unless a fallback says otherwise");
        assert_eq!(build_model_for(&config, "lonely", None).unwrap().provider_id(), "spare", "the one member that could be built, not the combo's name");

        let legacy = FileConfig { provider: Some(Provider::Openai), api_keys: ApiKeys { openai: Some("k".into()), ..Default::default() }, ..Default::default() };
        assert_eq!(resolve_model_provider(&legacy, &Overrides::default()).unwrap().provider_id(), "openai");
    }

    /// A server on a free local port that answers every request with `status` and an echoing body, from a thread of its own.
    fn answers_with(status: u16) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let body = r#"{"error":"Incorrect API key provided: sk-secret"}"#;
                let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            }
        });
        format!("http://{addr}/v1")
    }

    /// P10: testing a provider's key builds it the way a chat would and asks the provider; what can't be tested says why.
    #[tokio::test]
    async fn a_providers_key_is_tested_as_built_and_what_cannot_be_says_why() {
        let compatible = |id: &str, base_url: Option<String>, key: Option<&str>| ProviderConfig {
            id: id.to_string(),
            kind: Provider::OpenaiCompatible,
            api_key: key.map(str::to_string),
            base_url,
            model: Some("m".to_string()),
            node: None,
        };
        assert_eq!(test_provider(&compatible("local", Some(answers_with(200)), Some("sk-secret"))).await, KeyCheck::Accepted);
        let rejected = test_provider(&compatible("local", Some(answers_with(401)), Some("sk-secret"))).await;
        assert_eq!(rejected.kind(), "rejected");
        assert!(!rejected.message().contains("sk-secret") && !rejected.message().contains("Incorrect"), "{}", rejected.message());

        // Not even buildable: no address to ask. That is what the person is told, not "unreachable".
        let missing = test_provider(&compatible("local", None, None)).await;
        assert_eq!(missing.kind(), "rejected");
        assert!(missing.message().contains("base_url"), "{}", missing.message());

        let node = ProviderConfig { id: "casa".into(), kind: Provider::Node, api_key: None, base_url: None, model: Some("ollama".into()), node: Some("node-casa".into()) };
        assert_eq!(test_provider(&node).await.kind(), "unsupported");
    }

    #[test]
    fn a_combo_can_be_the_active_model() {
        let config = FileConfig {
            providers: vec![provider_entry("main", Provider::Gemini), provider_entry("spare", Provider::Anthropic)],
            combos: vec![combo("fast", &["main", "spare"])],
            active_provider: Some("fast".into()),
            ..Default::default()
        };
        assert!(resolve_model_provider(&config, &Overrides::default()).is_ok());
    }

    #[test]
    fn the_old_reserve_list_becomes_the_active_combo_and_a_save_writes_it_that_way() {
        let path = temp_toml_path("legacy-fallbacks");
        std::fs::write(
            &path,
            "active_provider = \"main\"\nfallback_providers = [\"spare\", \"ghost\", \"main\"]\n\n[[providers]]\nid = \"main\"\nkind = \"gemini\"\n\n[[providers]]\nid = \"spare\"\nkind = \"anthropic\"\n",
        )
        .unwrap();
        let config = load_config_from_path(&path, true).unwrap();
        assert_eq!(config.combos, vec![combo("main-reserva", &["main", "spare"])]);
        assert_eq!(config.active_provider.as_deref(), Some("main-reserva"));

        save_config(&path, &config).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("fallback_providers") && saved.contains("[[combos]]"), "{saved}");
        assert_eq!(load_config_from_path(&path, true).unwrap().combos, config.combos, "migrates once");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn renaming_or_removing_providers_and_combos_keeps_every_reference_whole() {
        let mut config = FileConfig {
            combos: vec![combo("fast", &["a", "b"]), combo("solo", &["b"])],
            active_provider: Some("solo".into()),
            agents: vec![AgentConfig {
                id: "pirate".into(),
                persona: "p".into(),
                provider_id: Some("fast".into()),
                can_delegate_to_agents: false,
                can_manage_agents: false,
                can_message_agents: false,
                can_manage_tasks: false,
                allowed_tools: None,
                autonomy: default_autonomy(),
                approval_required: Vec::new(),
                role: None,
                reports_to: None,
                owner: None,
                shared_with: Vec::new(),
                delegation_models: Vec::new(),
            }],
            ..Default::default()
        };
        rename_provider_cascade(&mut config, "a", "a2");
        assert_eq!(config.combos[0].providers, vec!["a2".to_string(), "b".to_string()]);

        // "solo" empties and goes, and the active model that named it with it.
        remove_provider_references(&mut config, "b");
        assert_eq!(config.combos, vec![combo("fast", &["a2"])]);
        assert_eq!(config.active_provider, None);

        rename_combo(&mut config, "fast", "quick");
        assert_eq!(config.agents[0].provider_id.as_deref(), Some("quick"));
        remove_combo(&mut config, "quick");
        assert!(config.combos.is_empty());
        assert_eq!(config.agents[0].provider_id, None);
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
        ProviderConfig { id: id.to_string(), kind: Provider::Gemini, api_key: None, base_url: None, model: None, node: None }
    }

    fn agent_config(id: &str, provider_id: Option<&str>) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            persona: String::new(),
            provider_id: provider_id.map(str::to_string),
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            autonomy: default_autonomy(),
            approval_required: Vec::new(),
            role: None,
            reports_to: None,
            owner: None,
            shared_with: Vec::new(),
            delegation_models: Vec::new(),
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
        let tool = build_delegate_to_agent_tool(&config, &orchestrator, None).unwrap();
        let _chief = orchestrator.with_allowed_tools(Some(&["read_file".to_string()]));
        let ask = |id: &str| serde_json::json!({ "agent_id": id, "task": "go" });
        assert_eq!(tool.call(ask("reader")).await.unwrap()["result"], "read_file");
        assert_eq!(tool.call(ask("ops")).await.unwrap()["result"], "shell");
        assert_eq!(tool.call(ask("open")).await.unwrap()["result"], "read_file,write_file,shell");
    }

    #[test]
    fn a_caller_in_the_organization_delegates_only_to_its_subordinates_and_one_outside_it_reaches_everyone() {
        use warden_core::model::{response_stream, ChatStream, Response};
        use warden_core::tool::ToolSpec;

        struct Silent;
        #[async_trait::async_trait]
        impl ModelProvider for Silent {
            async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                Ok(response_stream(Response { content: String::new(), tool_calls: Vec::new(), usage: None }))
            }
        }
        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-delegate-scope-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        let orchestrator = Orchestrator::new(Arc::new(Silent), vault);
        let under = |id: &str, boss: Option<&str>| AgentConfig { reports_to: boss.map(str::to_string), ..agent_config(id, None) };
        let config = FileConfig {
            agents: vec![under("boss", None), under("a", Some("boss")), under("a1", Some("a")), under("b", Some("boss")), under("solo", None)],
            ..FileConfig::default()
        };
        let reaches = |caller: Option<&str>| delegate_targets(&config, &orchestrator, caller).into_iter().map(|t| t.id).collect::<Vec<_>>();

        assert_eq!(reaches(Some("boss")), ["a", "a1", "b"], "everyone below it, at any depth, and not itself");
        assert_eq!(reaches(Some("a")), ["a1"], "its own branch only, not a peer's");
        assert!(reaches(Some("a1")).is_empty(), "a leaf has nobody below it");
        assert_eq!(reaches(Some("solo")), ["boss", "a", "a1", "b", "solo"], "outside the organization: as before");
        assert_eq!(reaches(None), ["boss", "a", "a1", "b", "solo"]);
    }

    /// Three agents in one script, told apart by the task they are given: `boss` ("go") delegates to `manager` ("manage") in the background, and
    /// `manager` delegates to `worker` ("work") in the background; each reads its job. Writes down what every turn was offered.
    struct Hierarchy {
        offered: Arc<std::sync::Mutex<Offers>>,
        /// The `model` the manager asks for when it delegates to the worker, if any.
        worker_model: Option<&'static str>,
    }

    /// What each turn was offered: its first message, and its tools with their parameters.
    type Offers = Vec<(String, Vec<(String, serde_json::Value)>)>;

    #[async_trait::async_trait]
    impl ModelProvider for Hierarchy {
        async fn chat_stream(
            &self,
            messages: Vec<Message>,
            tools: Vec<warden_core::tool::ToolSpec>,
        ) -> anyhow::Result<warden_core::model::ChatStream> {
            use warden_core::model::{response_stream, Response, Role, ToolCall};
            let first = messages.iter().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default();
            let results = messages.iter().filter(|m| m.role == Role::Tool).count();
            self.offered.lock().unwrap().push((first.clone(), tools.iter().map(|t| (t.name.clone(), t.parameters.clone())).collect()));
            let call = |name: &str, arguments: serde_json::Value| ToolCall { id: "c".into(), name: name.into(), arguments, thought_signature: None };
            let delegate = |to: &str, task: &str, model: Option<&str>| {
                let mut arguments = serde_json::json!({ "agent_id": to, "task": task, "background": true });
                if let Some(model) = model {
                    arguments["model"] = model.into();
                }
                call("delegate_to_agent", arguments)
            };
            let read = call("jobs", serde_json::json!({ "action": "result", "job_id": "job-1" }));
            let calls = match (first.as_str(), results) {
                ("go", 0) => vec![delegate("manager", "manage", None)],
                ("manage", 0) if tools.iter().any(|t| t.name == "delegate_to_agent") => vec![delegate("worker", "work", self.worker_model)],
                ("go" | "manage", 1) => vec![read],
                _ => Vec::new(),
            };
            let content = if calls.is_empty() { format!("{first} done") } else { String::new() };
            Ok(response_stream(Response { content, tool_calls: calls, usage: None }))
        }
    }

    /// `boss` (delegates) over `manager` (may delegate, unless `manager_delegates` is false) over `worker`, run from the boss's turn.
    async fn run_hierarchy(manager_delegates: bool) -> (Vec<agent_tasks::AgentTask>, Offers) {
        run_hierarchy_with(manager_delegates, None, FileConfig::default()).await
    }

    /// `run_hierarchy`, with the models of `extra` (providers, combos, policies) in the config and the manager asking for `worker_model`.
    async fn run_hierarchy_with(manager_delegates: bool, worker_model: Option<&'static str>, extra: FileConfig) -> (Vec<agent_tasks::AgentTask>, Offers) {
        let offered = Arc::new(std::sync::Mutex::new(Vec::new()));
        let dir = std::env::temp_dir().join(format!("warden-hierarchy-test-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let log = dir.join("agent_tasks.jsonl");
        let mut base = Orchestrator::new(Arc::new(Hierarchy { offered: offered.clone(), worker_model }), Arc::new(Vault::new(dir.join("vault"))));
        base.register_tool(Arc::new(JobsTool::new()));
        let base = base.with_parallel_jobs(2).with_task_recorder(Arc::new(agent_tasks::FileTaskRecorder::new(&log)));
        let under = |id: &str, boss: Option<&str>, delegates: bool| AgentConfig { reports_to: boss.map(str::to_string), can_delegate_to_agents: delegates, ..agent_config(id, None) };
        let config = FileConfig {
            agents: vec![under("boss", None, true), under("manager", Some("boss"), manager_delegates), under("worker", Some("manager"), false)],
            ..extra
        };
        let boss = scope_to_agent(&base, &config, None, "boss", AgentExtras::default()).unwrap();
        assert_eq!(boss.orchestrator.handle_message(&[], "go").await.unwrap().content, "go done");
        let offered = offered.lock().unwrap().clone();
        (agent_tasks::read_agent_tasks(&log), offered)
    }

    #[tokio::test]
    async fn a_manager_delegated_to_in_the_background_delegates_to_its_own_subordinates_and_the_tree_is_recorded() {
        let (tasks, offered) = run_hierarchy(true).await;

        let by = |name: &str| tasks.iter().find(|t| t.assignee == name).unwrap_or_else(|| panic!("no task for {name}: {tasks:?}"));
        let (manager, worker) = (by("manager"), by("worker"));
        assert_eq!((manager.parent_id.as_deref(), manager.owner.as_deref()), (None, Some("boss")));
        assert_eq!((worker.parent_id.as_deref(), worker.owner.as_deref(), worker.group.as_str()), (Some(manager.id.as_str()), Some("manager"), manager.group.as_str()));
        assert!(tasks.iter().all(|t| t.state == agent_tasks::TaskState::Done), "{tasks:?}");

        // The manager's own `delegate_to_agent` reaches the worker and only the worker (its subordinates, not the boss or itself).
        let manager_turn: Vec<&(String, serde_json::Value)> = offered.iter().filter(|(first, _)| first == "manage").flat_map(|(_, tools)| tools).collect();
        let delegate = manager_turn.iter().find(|(name, _)| name == "delegate_to_agent").expect("the manager was given delegate_to_agent");
        assert_eq!(delegate.1["properties"]["agent_id"]["enum"], serde_json::json!(["worker"]));
        assert!(delegate.1["properties"].get("background").is_some(), "and can start it in the background");

        // The worker's turn is the deepest: no delegation, no jobs.
        let worker_tools: Vec<String> = offered.iter().filter(|(first, _)| first == "work").flat_map(|(_, tools)| tools.iter().map(|(n, _)| n.clone())).collect();
        assert!(!worker_tools.iter().any(|n| n == "delegate_to_agent" || n == "jobs"), "{worker_tools:?}");
    }

    /// Two providers that never answer (nothing listens there) and a named policy over one of them, so a task can be given a model
    /// without any real call going out.
    fn models_with_policy() -> FileConfig {
        let local = |id: &str| ProviderConfig {
            id: id.to_string(),
            kind: Provider::OpenaiCompatible,
            api_key: None,
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            model: Some("m".to_string()),
            node: None,
        };
        FileConfig {
            providers: vec![local("small"), local("big")],
            model_policies: vec![ModelPolicyConfig { id: "reasoning".into(), model: "big".into(), description: "hard problems".into() }],
            ..FileConfig::default()
        }
    }

    #[test]
    fn a_named_policy_is_offered_beside_the_ids_with_what_it_is_for_and_answers_with_its_model() {
        let choices = model_choices(&models_with_policy()).expect("two providers and a policy");
        assert_eq!(choices.ids, ["small", "big", "reasoning"]);
        assert_eq!(choices.hints, [("reasoning".to_string(), "hard problems".to_string())]);
        assert!(warden_core::tool::delegate::model_property(&choices)["description"].as_str().unwrap().contains("- reasoning: hard problems"));
        assert_eq!((choices.resolve)("reasoning").unwrap().model_id(), "m");
        assert!((choices.resolve)("ghost").is_err());

        // A policy that clashes with an id, repeats one, or names nothing is left out; a lone provider plus a policy is a choice.
        let mut config = models_with_policy();
        let policy = |id: &str, model: &str| ModelPolicyConfig { id: id.into(), model: model.into(), description: String::new() };
        config.model_policies = vec![policy("small", "big"), policy("code", "ghost"), policy("fast", "small"), policy("fast", "big")];
        let choices = model_choices(&config).unwrap();
        assert_eq!((choices.ids.clone(), choices.hints.clone()), (vec!["small".to_string(), "big".into(), "fast".into()], vec![("fast".to_string(), "the same as 'small'".to_string())]));
        config.providers.truncate(1);
        config.model_policies = vec![policy("fast", "small")];
        assert_eq!(model_choices(&config).unwrap().ids, ["small", "fast"]);
        config.model_policies.clear();
        assert!(model_choices(&config).is_none(), "one model and no policy is no choice");
    }

    fn limited_to(models: &[&str]) -> AgentConfig {
        AgentConfig { delegation_models: models.iter().map(|m| m.to_string()).collect(), can_delegate_to_agents: true, ..agent_config("manager", None) }
    }

    #[test]
    fn a_limit_on_the_models_of_an_agent_offers_only_those_that_exist_and_the_first_is_the_default() {
        let config = models_with_policy();

        // No limit: everyone's choices, and nothing is the default.
        let open = model_choices_for(&config, Some(&limited_to(&[]))).unwrap();
        assert_eq!((open.ids.clone(), open.default.clone()), (vec!["small".to_string(), "big".into(), "reasoning".into()], None));
        assert_eq!(model_choices_for(&config, None).unwrap().ids.len(), 3);

        // A limit keeps the order given, drops what doesn't exist, and its first is the default.
        let limited = model_choices_for(&config, Some(&limited_to(&["reasoning", "ghost", "small"]))).unwrap();
        assert_eq!((limited.ids.clone(), limited.default.as_deref()), (vec!["reasoning".to_string(), "small".into()], Some("reasoning")));
        assert_eq!(limited.hints, [("reasoning".to_string(), "hard problems".to_string())], "only the hints of what is still offered");
        assert_eq!((limited.resolve)("reasoning").unwrap().model_id(), "m");

        // A list of one is the person dictating the model; one with nothing left offers no choice at all, never everything.
        let dictated = model_choices_for(&config, Some(&limited_to(&["small"]))).unwrap();
        assert_eq!((dictated.ids, dictated.default.as_deref()), (vec!["small".to_string()], Some("small")));
        assert!(model_choices_for(&config, Some(&limited_to(&["ghost"]))).is_none());

        // Even a hub with one provider can dictate its model.
        let mut one = models_with_policy();
        one.providers.truncate(1);
        one.model_policies.clear();
        assert!(model_choices(&one).is_none() && model_choices_for(&one, Some(&limited_to(&["small"]))).is_some());
    }

    #[tokio::test]
    async fn an_agent_limited_to_some_models_delegates_with_only_those_in_every_tool_it_has() {
        let mut config = models_with_policy();
        config.agents = vec![limited_to(&["reasoning", "small"]), agent_config("worker", None)];
        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!("warden-limit-models-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))));
        let mut base = Orchestrator::new(Arc::new(Hierarchy { offered: Default::default(), worker_model: None }), vault);
        base.register_tool(Arc::new(DelegateTool::new(base.clone()).with_models(model_choices(&config).unwrap())));

        let scoped = scope_to_agent(&base, &config, None, "manager", AgentExtras::default()).unwrap();

        for name in ["delegate_task", "delegate_to_agent"] {
            let spec = scoped.orchestrator.tools().iter().map(|t| t.spec()).find(|s| s.name == name).unwrap_or_else(|| panic!("no {name}"));
            let model = &spec.parameters["properties"]["model"];
            assert_eq!(model["enum"], serde_json::json!(["reasoning", "small"]), "{name}");
            assert!(model["description"].as_str().unwrap().contains("Leave it out to use 'reasoning'"), "{name}: {model}");
        }
        // An agent with no limit keeps seeing all of them.
        let mut open = config;
        open.agents[0].delegation_models.clear();
        let scoped = scope_to_agent(&base, &open, None, "manager", AgentExtras::default()).unwrap();
        let spec = scoped.orchestrator.tools().iter().map(|t| t.spec()).find(|s| s.name == "delegate_to_agent").unwrap();
        assert_eq!(spec.parameters["properties"]["model"]["enum"], serde_json::json!(["small", "big", "reasoning"]));
    }

    #[test]
    fn what_an_agent_may_delegate_with_follows_a_renamed_model_and_forgets_a_removed_one() {
        let mut config = models_with_policy();
        config.agents = vec![limited_to(&["reasoning", "small", "big"])];

        rename_provider_cascade(&mut config, "small", "tiny");
        assert_eq!(config.agents[0].delegation_models, ["reasoning", "tiny", "big"]);

        // `big` goes, and so does `reasoning`, the policy that answered with it.
        remove_provider_references(&mut config, "big");
        assert_eq!(config.agents[0].delegation_models, ["tiny"]);
        assert!(config.model_policies.is_empty());

        // Everything it listed gone: the list is empty, which means open again.
        remove_provider_references(&mut config, "tiny");
        assert!(config.agents[0].delegation_models.is_empty());
    }

    #[test]
    fn a_policy_follows_the_model_it_names_when_that_is_renamed_and_goes_when_it_is_removed() {
        let mut config = models_with_policy();
        rename_provider_cascade(&mut config, "big", "bigger");
        assert_eq!(config.model_policies[0].model, "bigger");
        remove_provider_references(&mut config, "bigger");
        assert!(config.model_policies.is_empty());

        let mut config = models_with_policy();
        config.model_policies.push(ModelPolicyConfig { id: "fast".into(), model: "mix".into(), description: String::new() });
        config.combos = vec![combo("mix", &["small"])];
        rename_combo(&mut config, "mix", "blend");
        assert_eq!(config.model_policies[1].model, "blend");
        remove_combo(&mut config, "blend");
        assert_eq!(config.model_policies.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["reasoning"]);
    }

    #[test]
    fn model_policies_are_read_from_config_toml_and_written_back() {
        let text = "[[providers]]\nid = \"a\"\nkind = \"gemini\"\n[[model_policies]]\nid = \"cheap\"\nmodel = \"a\"\ndescription = \"simple work\"\n";
        let config: FileConfig = toml::from_str(text).unwrap();
        assert_eq!(config.model_policies, [ModelPolicyConfig { id: "cheap".into(), model: "a".into(), description: "simple work".into() }]);
        assert!(toml::to_string(&config).unwrap().contains("[[model_policies]]"));
        assert!(!toml::to_string(&FileConfig::default()).unwrap().contains("model_policies"));
    }

    /// P123: the manager reached in the background is offered the models and the policies too, and the one it picks for its own subtask is
    /// the one recorded on that subtask — the choice holds down the whole chain, not only at the first level.
    #[tokio::test]
    async fn the_model_a_nested_agent_chooses_for_its_subtask_is_offered_and_recorded() {
        let (tasks, offered) = run_hierarchy_with(true, Some("reasoning"), models_with_policy()).await;

        let manager_tools: Vec<&(String, serde_json::Value)> = offered.iter().filter(|(first, _)| first == "manage").flat_map(|(_, tools)| tools).collect();
        let delegate = manager_tools.iter().find(|(name, _)| name == "delegate_to_agent").expect("the manager was given delegate_to_agent");
        assert_eq!(delegate.1["properties"]["model"]["enum"], serde_json::json!(["small", "big", "reasoning"]));

        let by = |name: &str| tasks.iter().find(|t| t.assignee == name).unwrap_or_else(|| panic!("no task for {name}: {tasks:?}"));
        assert_eq!(by("worker").model.as_deref(), Some("reasoning"));
        assert_eq!(by("manager").model, None, "the boss chose none for the manager");
    }

    #[tokio::test]
    async fn an_agent_that_may_not_delegate_is_not_given_the_tool_in_its_background_task() {
        let (tasks, offered) = run_hierarchy(false).await;

        assert!(!offered.iter().filter(|(first, _)| first == "manage").flat_map(|(_, tools)| tools).any(|(name, _)| name == "delegate_to_agent"));
        assert!(!tasks.iter().any(|t| t.assignee == "worker"), "nothing was delegated to the worker: {tasks:?}");
        assert_eq!(tasks.len(), 1);
    }

    #[tokio::test]
    async fn an_agent_that_may_delegate_gets_the_tool_only_when_a_task_runs_not_when_the_targets_are_built() {
        // The spawner is lazy: building the targets must not build anyone's own delegation tool.
        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!("warden-lazy-delegation-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))));
        let orchestrator = Orchestrator::new(Arc::new(Hierarchy { offered: Default::default(), worker_model: None }), vault);
        let config = FileConfig { agents: vec![AgentConfig { can_delegate_to_agents: true, ..agent_config("manager", None) }, agent_config("plain", None)], ..FileConfig::default() };
        let targets = delegate_targets(&config, &orchestrator, None);
        let by = |id: &str| targets.iter().find(|t| t.id == id).unwrap();
        assert!(by("manager").delegation.is_some(), "an agent that may delegate can build its tool when a task of it runs");
        assert!(by("plain").delegation.is_none());
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
        let delegate = build_live_delegate_to_agent_tool(&path, &config, &orchestrator, revision.clone(), None).unwrap();
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
        let chief = orchestrator.with_tool(build_delegate_to_agent_tool(&config, &orchestrator, None).unwrap());

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

    /// P94: a new vault starts empty. The three files P52 used to seed in the root (`_profile.md` and its
    /// siblings) are gone, and one that already exists is left exactly as it is.
    #[test]
    fn a_new_vault_gets_no_seeded_files_and_an_old_one_keeps_its_own() {
        let dir = temp_dir("no-seed");
        let vault = Vault::new(dir.clone());
        assert!(vault.list_all_files().unwrap().is_empty());
        vault.write("_profile.md", "already customized by the user").unwrap();
        assert_eq!(vault.read("_profile.md").unwrap(), "already customized by the user");
        std::fs::remove_dir_all(&dir).ok();
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
