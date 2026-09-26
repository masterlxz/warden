use serde::{Deserialize, Serialize};
use serde_json::Value;
use warden_core::model::{Attachment, Usage};
use warden_core::skill::Skill;
use warden_core::spend::{LimitStatus, Price, SpendBreakdown, SpendBucket, SpendGuard};
use warden_core::tool::ToolSpec;

/// A skill (P16) on the wire — what the browser extension's Skills screen lists and edits over
/// `ListSkills`/`SaveSkill`. `agents` is `#[serde(default)]` so a client that never touches the
/// agent restriction (P72 c) can omit it; the server keeps the stored restriction on an edit then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub agents: Vec<String>,
}

impl From<Skill> for SkillDto {
    fn from(skill: Skill) -> Self {
        Self { name: skill.name, description: skill.description, body: skill.body, agents: skill.agents }
    }
}

/// Who said a message in a `History` reply (P40) — the same two roles the persisted conversation
/// ever holds (`warden_bootstrap::ChatRole`, which this crate can't depend on without a cycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryRole {
    User,
    Assistant,
}

/// One persisted message of this device's conversation, as sent back by `History` (P40). Only
/// what a chat transcript renders — usage/generated file paths stay on the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    pub role: HistoryRole,
    pub content: String,
    pub created_at: i64,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// One of a device's conversations in a `ConversationList` (P78) — enough for a sidebar; the
/// messages themselves come from `RequestHistory`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One line matching a `SearchVault` query (P78) — `warden_core::memory::SearchHit` on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSearchHit {
    pub path: String,
    pub line_number: usize,
    pub line: String,
}

/// One device's share of a `UsageReport` (P78). `name` comes from the hub's device registry; `None`
/// for a device it no longer knows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceUsage {
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub conversation_count: usize,
    pub message_count: usize,
    pub usage: Usage,
}

/// Model calls on one day of a `UsageReport`, `date` being `YYYY-MM-DD` in the viewer's time zone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsageDto {
    pub date: String,
    pub calls: usize,
    pub tokens: u64,
}

/// Where one spending limit (P4) stands — `warden_core::spend::LimitStatus` on the wire, plus what
/// one `ExtendLimit` would add (`extend_tokens`/`extend_cost_usd`, 0 for a ceiling it doesn't have).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitStatusDto {
    pub id: String,
    pub scope: String,
    pub window_hours: u32,
    pub used_tokens: u64,
    pub max_tokens: Option<u64>,
    pub used_cost_usd: f64,
    pub max_cost_usd: Option<f64>,
    /// The closer ceiling's share used; 1 or more is exhausted.
    pub fraction: f64,
    pub warn: bool,
    pub exceeded: bool,
    pub unpriced_calls: u64,
    pub frees_up_in_minutes: Option<u64>,
    pub extend_tokens: u64,
    pub extend_cost_usd: f64,
}

impl LimitStatusDto {
    /// `extension` is `SpendGuard::extension_size` for this limit.
    pub fn new(status: LimitStatus, (extend_tokens, extend_cost_usd): (u64, f64)) -> Self {
        Self {
            id: status.id,
            scope: status.scope,
            window_hours: status.window_hours,
            used_tokens: status.used_tokens,
            max_tokens: status.max_tokens,
            used_cost_usd: status.used_cost_usd,
            max_cost_usd: status.max_cost_usd,
            fraction: status.fraction,
            warn: status.warn,
            exceeded: status.exceeded,
            unpriced_calls: status.unpriced_calls,
            frees_up_in_minutes: status.frees_up_in_minutes,
            extend_tokens,
            extend_cost_usd,
        }
    }

    /// Every configured limit, as `guard` sees it now.
    pub fn all(guard: &SpendGuard) -> Vec<Self> {
        guard
            .status(None)
            .into_iter()
            .map(|status| {
                let extension = guard.extension_size(&status.id).unwrap_or_default();
                Self::new(status, extension)
            })
            .collect()
    }
}

impl From<SpendBucket> for SpendBucketDto {
    fn from(b: SpendBucket) -> Self {
        Self { key: b.key, calls: b.calls, tokens: b.tokens, cost_usd: b.cost_usd, unpriced_calls: b.unpriced_calls }
    }
}

impl From<SpendBreakdown> for RecentSpendDto {
    fn from(b: SpendBreakdown) -> Self {
        Self {
            window_hours: b.window_hours,
            by_model: b.by_model.into_iter().map(Into::into).collect(),
            by_channel: b.by_channel.into_iter().map(Into::into).collect(),
        }
    }
}

/// One model or channel in the ledger's recent spending (`warden_core::spend::SpendBucket`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendBucketDto {
    pub key: String,
    pub calls: u64,
    pub tokens: u64,
    pub cost_usd: f64,
    pub unpriced_calls: u64,
}

/// The ledger's recent spending, which is where dollars and models live: it only reaches back
/// `window_hours` (the longest limit), and covers every channel on the hub's machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentSpendDto {
    pub window_hours: u32,
    pub by_model: Vec<SpendBucketDto>,
    pub by_channel: Vec<SpendBucketDto>,
}

/// Reply body of `RequestUsage` (P78): tokens from every conversation the hub keeps (all devices,
/// all time), and the spending limits and recent dollars from the P4 ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReportDto {
    pub total: Usage,
    pub conversation_count: usize,
    pub message_count: usize,
    pub by_device: Vec<DeviceUsage>,
    pub daily: Vec<DailyUsageDto>,
    /// False when the hub runs with spending limits switched off — `limits` and `recent` are empty.
    pub limits_enabled: bool,
    pub limits: Vec<LimitStatusDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent: Option<RecentSpendDto>,
    /// Why the ledger couldn't be written the last time it failed — the numbers may be short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_error: Option<String>,
}

/// Whether a secret (an API key) is saved, without the secret itself: the hub never sends one back
/// (P78). `hint` is its last four characters, only for a secret long enough that they give nothing
/// away, so the person can tell which key is there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatusDto {
    pub set: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A device's standing in the hub's pairing registry (Fase 9.3) — mirrors `warden-server`'s
/// `PairingStatus`, which this crate can't see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceStatusDto {
    Pending,
    Approved,
    Revoked,
}

/// One device that has ever said `Hello` to the hub, for the web's device list (Sessão 103).
/// Never its token or token hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDto {
    pub device_id: String,
    pub device_name: String,
    pub status: DeviceStatusDto,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

/// What `SetDeviceStatus` does — the same two actions as `warden-server devices approve|revoke`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceAction {
    Approve,
    Revoke,
}

/// What a settings save does to one secret. `Keep` is what an untouched field sends, since the
/// client never had the value to send back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "camelCase")]
pub enum SecretEdit {
    Keep,
    Set(String),
    Clear,
}

impl SecretEdit {
    pub fn is_set(&self) -> bool {
        matches!(self, SecretEdit::Set(_))
    }
}

/// One model provider as the settings screen shows it. `kind` is the config's own spelling
/// (`gemini`, `openai`, `anthropic`, `openai_compatible`); empty strings mean "not set".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsDto {
    pub id: String,
    pub kind: String,
    pub base_url: String,
    pub model: String,
    pub api_key: SecretStatusDto,
}

/// One model provider as a save sends it. `original_id` is the id it had when the screen loaded
/// (absent for a new one): that is how `SecretEdit::Keep` finds the saved key of a renamed provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEditDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_id: Option<String>,
    pub id: String,
    pub kind: String,
    pub base_url: String,
    pub model: String,
    pub api_key: SecretEdit,
}

/// One agent, both ways. `original_id` only matters in a save (see `ProviderEditDto`): it carries a
/// rename into the SSH hosts that name the agent, which the web screen doesn't show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettingsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_id: Option<String>,
    pub id: String,
    pub persona: String,
    /// Empty for "no default model".
    pub provider_id: String,
    pub can_delegate_to_agents: bool,
    pub can_manage_agents: bool,
    /// `None` keeps every tool.
    pub allowed_tools: Option<Vec<String>>,
}

/// One `[[limits]]` entry (P4) as a settings form edits it. `scope` is `global`, `agent`, `channel`
/// or `user`; `target` is empty for a global limit; `warn_at`/`extend_step` are fractions (0–1),
/// `None` meaning the default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitSettingsDto {
    pub id: String,
    pub scope: String,
    pub target: String,
    pub window_hours: u32,
    pub max_tokens: Option<u64>,
    pub max_cost_usd: Option<f64>,
    pub warn_at: Option<f64>,
    pub extend_step: Option<f64>,
}

/// One `[[prices]]` entry: dollars per million tokens for the model with exactly this id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceSettingsDto {
    pub model: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

impl From<Price> for PriceSettingsDto {
    fn from(p: Price) -> Self {
        Self { model: p.model, input_per_mtok: p.input_per_mtok, output_per_mtok: p.output_per_mtok }
    }
}

/// The part of the hub's `config.toml` the web settings screen shows (P78): providers, agents, the
/// Tavily/Whisper keys, spending limits and prices. Shell, MCP servers, SSH hosts, storage and paths
/// stay off it on purpose, since they would let a paired device run commands on the hub's machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubSettingsDto {
    pub providers: Vec<ProviderSettingsDto>,
    /// Empty when none is picked.
    pub active_provider: String,
    pub agents: Vec<AgentSettingsDto>,
    pub tavily_key: SecretStatusDto,
    pub whisper_key: SecretStatusDto,
    /// `None` means no `[[limits]]` in the file, so the built-in `default_limits` apply.
    pub limits: Option<Vec<LimitSettingsDto>>,
    pub default_limits: Vec<LimitSettingsDto>,
    /// `WARDEN_SPEND_LIMITS=off` in the hub's environment beats whatever the file says.
    pub limits_disabled_by_env: bool,
    pub prices: Vec<PriceSettingsDto>,
    /// Provider kind → the model used when a provider leaves `model` empty.
    pub default_models: std::collections::BTreeMap<String, String>,
    /// Every tool the hub's orchestrator has, for an agent's allowed-tools list.
    pub tool_names: Vec<String>,
    /// Things outside the file that change what it means on this hub (a `--provider` flag, a
    /// providers list still empty, ...), one sentence each.
    pub notes: Vec<String>,
}

/// A settings save: the whole editable part, replacing what the file has for it. Everything the
/// screen doesn't show is kept as it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubSettingsUpdate {
    pub providers: Vec<ProviderEditDto>,
    pub active_provider: String,
    pub agents: Vec<AgentSettingsDto>,
    pub tavily_key: SecretEdit,
    pub whisper_key: SecretEdit,
    /// `None` goes back to the built-in limits, `Some(vec![])` turns every limit off.
    pub limits: Option<Vec<LimitSettingsDto>>,
    pub prices: Vec<PriceSettingsDto>,
}

impl HubSettingsUpdate {
    /// Whether this save carries a new secret, which the hub only accepts over an encrypted or local
    /// connection.
    pub fn sets_a_secret(&self) -> bool {
        self.tavily_key.is_set() || self.whisper_key.is_set() || self.providers.iter().any(|p| p.api_key.is_set())
    }
}

/// Messages sent from a client (mobile, desktop-as-client, browser extension) to the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClientMessage {
    Hello {
        device_id: String,
        device_name: String,
        /// The hub's shared *pairing* key (P36) — only needed to pair (no `device_token` yet, or
        /// the one held was rejected). May be empty when `device_token` is set.
        #[serde(default)]
        auth_key: String,
        /// The per-device token this hub issued in an earlier `HelloAck` (P36). Keeps working
        /// after the operator rotates the pairing key; stops working once the device is revoked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_token: Option<String>,
        /// Local tools this client can execute on request (Fase 7.4) — e.g. mobile's
        /// `list_files`/`read_file`. `#[serde(default)]` so a client that predates this (or
        /// simply has none configured, like the desktop-as-client) doesn't need to send anything;
        /// `Server` only builds the remote-tool-dispatch machinery when this is non-empty.
        #[serde(default)]
        tools: Vec<ToolSpec>,
    },
    Ping {
        nonce: u64,
    },
    /// A chat turn (Fase 7.3) — answered by the `Orchestrator` `Server` now hosts, appended to one
    /// of this device's conversations. `conversation_id` picks which (P78): an id the hub has never
    /// seen starts a new conversation, titled from this first message; `None` means the device's
    /// default conversation, the only one a client from before P78 ever used.
    Chat {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
        /// Images or PDFs sent with this turn (P78) — the mime types in
        /// `warden_core::model::USER_ATTACHMENT_MIME_TYPES`, within the hub's size cap; anything
        /// else gets a `ChatError`. With attachments, `message` may be empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
    /// The result of a `ServerMessage::ToolCallRequest` this client was asked to run (Fase 7.4).
    ToolCallResult {
        call_id: u64,
        result: Value,
    },
    /// The client failed to run a requested tool call (Fase 7.4) — same "carry the real error
    /// text" posture as `ServerMessage::ChatError`.
    ToolCallError {
        call_id: u64,
        message: String,
    },
    /// Asks the server to route a tool call to a *different* connected device (Fase 9.3/9.4) —
    /// unlike `Hello.tools`/`ToolCallRequest` (Fase 7.4, always a round-trip back to the same
    /// connection that advertised the tool), this lets any connected client reach a specific
    /// other one by `target_device_id`. `call_id` is this connection's own id (allocated the same
    /// way `Ping`'s `nonce` is, by the caller) — echoed back on the matching
    /// `ServerMessage::DeviceToolResult`/`DeviceToolError` so concurrent calls stay correlated.
    /// The server never inspects `tool`/`arguments`; only the target device's own code decides
    /// what they mean.
    CallDeviceTool {
        call_id: u64,
        target_device_id: String,
        tool: String,
        arguments: Value,
    },
    /// Skills management (P72) — list the skills in the vault this server hosts. `request_id` is
    /// the caller's own correlation id (same idea as `CallDeviceTool.call_id`), echoed back on the
    /// matching `SkillList`/`SkillError` so concurrent requests stay paired.
    ListSkills {
        request_id: u64,
    },
    /// Creates (`overwrite: false`, refuses a taken name) or edits (`overwrite: true`) a skill.
    /// An edit whose `skill.agents` is empty keeps the agent restriction already on disk — a client
    /// with no UI for it must not silently make a restricted skill global again.
    SaveSkill {
        request_id: u64,
        skill: SkillDto,
        overwrite: bool,
    },
    DeleteSkill {
        request_id: u64,
        name: String,
    },
    /// Asks for this device's persisted conversation (P40) — the one `Chat` turns are appended to,
    /// keyed by `Hello.device_id`, so a client that reconnects (or was restarted) can show what was
    /// already said. Answered by `History`/`HistoryError` with the same `request_id`. `limit` keeps
    /// only the most recent messages; `None` returns all of them.
    RequestHistory {
        request_id: u64,
        #[serde(default)]
        limit: Option<u32>,
        /// Which conversation (P78) — `None` is the device's default one, same as `Chat`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    /// Lists this device's conversations (P78), newest-updated first. Answered by
    /// `ConversationList`/`ConversationError` with the same `request_id`.
    ListConversations {
        request_id: u64,
    },
    /// Renames one of this device's conversations. Answered by `ConversationOk`/`ConversationError`.
    RenameConversation {
        request_id: u64,
        conversation_id: String,
        title: String,
    },
    /// Deletes one of this device's conversations. Answered by `ConversationOk`/`ConversationError`.
    DeleteConversation {
        request_id: u64,
        conversation_id: String,
    },
    /// Voice input (P78): a recording the hub transcribes with Whisper, the same way the desktop's
    /// mic button does — the text comes back in `Transcription` for the client to put in its
    /// composer, nothing is sent to the model. `TranscriptionError` when the hub has no Whisper key
    /// or the call fails.
    Transcribe {
        request_id: u64,
        audio: Attachment,
    },
    /// The vault this hub hosts, as a person browses it (P78) — every file except the fixed ones at
    /// the root, `skills/` and dotfiles (`Vault::browse_files`). Answered by `VaultFileList`.
    ListVaultFiles {
        request_id: u64,
    },
    /// Opens one note, answered by `VaultNote` with the version to send back when saving it.
    ReadVaultNote {
        request_id: u64,
        path: String,
    },
    /// Saves a note. `expected_version: None` creates it (refused if the path is taken); `Some` is
    /// an edit of the version that was opened, refused with `VaultError { conflict: true }` if the
    /// note changed since. Answered by `VaultSaved`.
    SaveVaultNote {
        request_id: u64,
        path: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_version: Option<String>,
    },
    /// Deletes a note unless it changed since `expected_version`. Answered by `VaultOk`.
    DeleteVaultNote {
        request_id: u64,
        path: String,
        expected_version: String,
    },
    /// Word search over the vault's notes (`Vault::search`), answered by `VaultSearchResults`.
    SearchVault {
        request_id: u64,
        query: String,
    },
    /// Token and spending usage across the whole hub (P78), answered by `UsageReport`.
    /// `tz_offset_minutes` is the viewer's offset from UTC (UTC−3 is `-180`), so the daily series
    /// splits days at the viewer's midnight.
    RequestUsage {
        request_id: u64,
        #[serde(default)]
        tz_offset_minutes: i32,
    },
    /// Lets one spending limit go one `extend_step` further for the rest of its window — what the
    /// desktop's pause dialog does, for a client that has no way to be asked mid-turn. Answered by
    /// `LimitExtended` with the limit's new standing.
    ExtendLimit {
        request_id: u64,
        limit_id: String,
    },
    /// The hub's editable settings (P78), answered by `Settings`.
    RequestSettings {
        request_id: u64,
    },
    /// Replaces the editable settings and reloads the hub's orchestrator with them. `pairing_key` is
    /// asked again on every save, so a leaked device token alone can't swap API keys.
    /// `base_version` is the `Settings.version` the screen loaded; a file changed since then is a
    /// conflict, not an overwrite.
    SaveSettings {
        request_id: u64,
        pairing_key: String,
        base_version: String,
        update: HubSettingsUpdate,
    },
    /// Every device in the hub's pairing registry, answered by `DeviceList` — what
    /// `warden-server devices list` prints, so a hub with no screen can be managed from a browser.
    ListDevices {
        request_id: u64,
    },
    /// Approves or revokes a device, answered by the updated `DeviceList`. `pairing_key` is asked
    /// every time, as in `SaveSettings`, so a leaked device token alone can't let a new device in
    /// or lock the owner's out.
    SetDeviceStatus {
        request_id: u64,
        pairing_key: String,
        device_id: String,
        action: DeviceAction,
    },
    /// An unauthenticated presence probe (Fase 9.1 redefined — LAN discovery, not the
    /// authenticated connection Hello starts). No `auth_key`/`device_id` on purpose: the whole
    /// point is finding a hub *before* knowing its credential. Answered by `DiscoverAck` and the
    /// connection closes right after — never reaches `Hello`'s device-registry bookkeeping.
    Discover,
    Goodbye {
        reason: Option<String>,
    },
}

impl ClientMessage {
    /// A plain text `Chat` turn to the default conversation, with no attachments.
    pub fn chat(message: impl Into<String>) -> Self {
        ClientMessage::Chat { message: message.into(), conversation_id: None, attachments: Vec::new() }
    }
}

/// Messages sent from the server to a connected client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ServerMessage {
    HelloAck {
        server_name: String,
        /// A newly issued per-device token (P36), present whenever this `Hello` paired with the
        /// pairing key instead of an existing token. The client must store it (keyed by hub and
        /// `device_id`) and send it as `Hello.device_token` from then on — it replaces any token
        /// it held before, which no longer works.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_token: Option<String>,
    },
    AuthError {
        reason: String,
    },
    Pong {
        nonce: u64,
    },
    /// The model's answer to a `Chat` message.
    ChatResponse {
        content: String,
        usage: Option<Usage>,
        /// Media extracted from an MCP tool result during this turn (P64 frente 2 fatia 3).
        /// `#[serde(default)]` so a peer from before this field existed (older client build, or a
        /// stored fixture) still parses.
        #[serde(default)]
        attachments: Vec<Attachment>,
        /// The conversation this answer belongs to (P78) — the `Chat.conversation_id` it answers,
        /// resolved (the default conversation's id when that was `None`). `Chat` carries no request
        /// id, so this is what lets a client with several conversations route a late answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    /// A `Chat` message failed (missing API key, rate limit, provider error, ...) — the raw error
    /// text, since this protocol has no untrusted-public-bot audience to hide it from.
    ChatError {
        message: String,
        /// Same as `ChatResponse.conversation_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
        /// Set when the turn stopped on a spending limit (P4) with no room left: that limit's id, so
        /// the client can offer to `ExtendLimit` it and send again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spend_limit_id: Option<String>,
    },
    /// Asks a connected client to run one of the tools it advertised in `Hello.tools` (Fase 7.4).
    /// `call_id` is scoped to this connection (a simple counter, mirrors `Ping`'s `nonce`) — the
    /// client echoes it back on `ToolCallResult`/`ToolCallError` so the server can correlate the
    /// reply even if several calls are in flight at once.
    ToolCallRequest {
        call_id: u64,
        tool: String,
        arguments: Value,
    },
    /// Reply to a `ClientMessage::CallDeviceTool` (Fase 9.4) — the target device answered. Same
    /// `call_id` the caller allocated for that request.
    DeviceToolResult {
        call_id: u64,
        result: Value,
    },
    /// A routed `CallDeviceTool` didn't succeed — covers both "the target device isn't connected"
    /// and "the target device ran the tool but it failed", same "one error variant, descriptive
    /// text" posture as `ChatError`; the caller has no separate branch to handle differently
    /// between those two cases anyway.
    DeviceToolError {
        call_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::ListSkills`.
    SkillList {
        request_id: u64,
        skills: Vec<SkillDto>,
    },
    /// Reply to a successful `SaveSkill`/`DeleteSkill`.
    SkillOk {
        request_id: u64,
    },
    /// A `ListSkills`/`SaveSkill`/`DeleteSkill` failed (invalid skill, name taken, no such skill) —
    /// the raw error text, same posture as `ChatError`.
    SkillError {
        request_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::RequestHistory`, oldest message first. Empty when this device never
    /// chatted before.
    History {
        request_id: u64,
        messages: Vec<HistoryMessage>,
    },
    /// The conversation file exists but couldn't be read/parsed — the raw error text, same posture
    /// as `SkillError`.
    HistoryError {
        request_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::ListConversations`.
    ConversationList {
        request_id: u64,
        conversations: Vec<ConversationSummary>,
    },
    /// Reply to a successful `RenameConversation`/`DeleteConversation`.
    ConversationOk {
        request_id: u64,
    },
    /// A `ListConversations`/`RenameConversation`/`DeleteConversation` failed (invalid id, no such
    /// conversation, unreadable directory) — the raw error text, same posture as `SkillError`.
    ConversationError {
        request_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::Transcribe`.
    Transcription {
        request_id: u64,
        text: String,
    },
    TranscriptionError {
        request_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::ListVaultFiles` — relative paths with `/` separators, sorted.
    VaultFileList {
        request_id: u64,
        files: Vec<String>,
    },
    /// Reply to `ClientMessage::ReadVaultNote`.
    VaultNote {
        request_id: u64,
        path: String,
        content: String,
        version: String,
    },
    /// Reply to a successful `SaveVaultNote`, with the note's new version.
    VaultSaved {
        request_id: u64,
        version: String,
    },
    /// Reply to a successful `DeleteVaultNote`.
    VaultOk {
        request_id: u64,
    },
    /// Reply to `ClientMessage::SearchVault`.
    VaultSearchResults {
        request_id: u64,
        hits: Vec<VaultSearchHit>,
    },
    /// A vault request failed. `conflict` is set when the note changed, appeared or was deleted
    /// since it was opened — the client should offer to reload rather than just show the text.
    VaultError {
        request_id: u64,
        message: String,
        #[serde(default)]
        conflict: bool,
    },
    /// Reply to `ClientMessage::RequestUsage`.
    UsageReport {
        request_id: u64,
        report: UsageReportDto,
    },
    /// Reply to a successful `ClientMessage::ExtendLimit`.
    LimitExtended {
        request_id: u64,
        limit: LimitStatusDto,
    },
    /// A `RequestUsage`/`ExtendLimit` failed (unreadable conversations, no such limit, limits off).
    UsageError {
        request_id: u64,
        message: String,
    },
    /// Reply to `ClientMessage::RequestSettings`. `version` goes back in `SaveSettings.base_version`.
    /// `secrets_writable` is false on a plain `http://` connection from another machine, where the
    /// hub refuses a new API key.
    Settings {
        request_id: u64,
        settings: HubSettingsDto,
        version: String,
        secrets_writable: bool,
    },
    /// Reply to a successful `ClientMessage::SaveSettings`, with what the file holds now.
    SettingsSaved {
        request_id: u64,
        settings: HubSettingsDto,
        version: String,
    },
    /// A settings request failed. `conflict`: the file changed since it was loaded. `auth_rejected`:
    /// the pairing key was wrong. Nothing was written in either case.
    SettingsError {
        request_id: u64,
        message: String,
        #[serde(default)]
        conflict: bool,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// Reply to `ListDevices` and to a successful `SetDeviceStatus`, sorted by id. `you` is the
    /// asking connection's own device id, so the screen can mark it.
    DeviceList {
        request_id: u64,
        devices: Vec<DeviceDto>,
        you: String,
    },
    /// A device request failed. `auth_rejected`: the pairing key was wrong; nothing changed.
    DeviceError {
        request_id: u64,
        message: String,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// Reply to `ClientMessage::Discover` — just enough for a sweeping client to show the operator
    /// "which machine is this" and let them pick it, never a secret.
    DiscoverAck {
        server_name: String,
        /// Set when this hub only accepts encrypted connections (P36): the `wss://` URL (a name
        /// its certificate is valid for, e.g. the Tailscale MagicDNS one) a client must use for
        /// `Hello` — a plain `ws://` Hello is refused before the upgrade even completes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secure_url: Option<String>,
    },
    Goodbye {
        reason: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_round_trips_through_json() {
        let msg = ClientMessage::Hello {
            device_id: "dev-1".into(),
            device_name: "Test Device".into(),
            auth_key: "secret".into(),
            device_token: None,
            tools: Vec::new(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret","tools":[]}"#
        );
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn client_hello_with_only_a_device_token_parses() {
        let json = r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","deviceToken":"tok"}"#;
        let msg = serde_json::from_str::<ClientMessage>(json).unwrap();
        assert!(matches!(msg, ClientMessage::Hello { auth_key, device_token: Some(t), .. } if auth_key.is_empty() && t == "tok"));
    }

    #[test]
    fn server_hello_ack_carries_an_issued_token_only_when_there_is_one() {
        let without = ServerMessage::HelloAck { server_name: "Hub".into(), device_token: None };
        assert_eq!(serde_json::to_string(&without).unwrap(), r#"{"type":"helloAck","serverName":"Hub"}"#);

        let with = ServerMessage::HelloAck { server_name: "Hub".into(), device_token: Some("tok".into()) };
        let json = serde_json::to_string(&with).unwrap();
        assert_eq!(json, r#"{"type":"helloAck","serverName":"Hub","deviceToken":"tok"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), with);
    }

    #[test]
    fn client_hello_without_a_tools_field_defaults_to_empty() {
        // A client written before Fase 7.4 (or one with nothing to advertise) never sends `tools`
        // at all — must still parse, not error.
        let json = r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret"}"#;
        let msg = serde_json::from_str::<ClientMessage>(json).unwrap();
        assert!(matches!(msg, ClientMessage::Hello { tools, .. } if tools.is_empty()));
    }

    #[test]
    fn client_hello_with_advertised_tools_round_trips_through_json() {
        let msg = ClientMessage::Hello {
            device_id: "dev-1".into(),
            device_name: "Test Device".into(),
            auth_key: "secret".into(),
            device_token: None,
            tools: vec![ToolSpec {
                name: "list_files".into(),
                description: "List files".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"hello","deviceId":"dev-1","deviceName":"Test Device","authKey":"secret","tools":[{"name":"list_files","description":"List files","parameters":{"type":"object"}}]}"#
        );
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn client_tool_call_result_round_trips_through_json() {
        let msg = ClientMessage::ToolCallResult { call_id: 7, result: serde_json::json!({"ok": true}) };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"toolCallResult","callId":7,"result":{"ok":true}}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn client_tool_call_error_round_trips_through_json() {
        let msg = ClientMessage::ToolCallError { call_id: 7, message: "boom".into() };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"toolCallError","callId":7,"message":"boom"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_auth_error_round_trips_through_json() {
        let msg = ServerMessage::AuthError {
            reason: "invalid auth key".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"authError","reason":"invalid auth key"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn client_call_device_tool_round_trips_through_json() {
        let msg = ClientMessage::CallDeviceTool {
            call_id: 1,
            target_device_id: "dev-2".into(),
            tool: "vault_read".into(),
            arguments: serde_json::json!({"path": "notes/a.md"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"callDeviceTool","callId":1,"targetDeviceId":"dev-2","tool":"vault_read","arguments":{"path":"notes/a.md"}}"#
        );
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_device_tool_result_round_trips_through_json() {
        let msg = ServerMessage::DeviceToolResult { call_id: 1, result: serde_json::json!({"content": "hi"}) };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"deviceToolResult","callId":1,"result":{"content":"hi"}}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_device_tool_error_round_trips_through_json() {
        let msg = ServerMessage::DeviceToolError { call_id: 1, message: "device 'dev-2' is not connected".into() };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"deviceToolError","callId":1,"message":"device 'dev-2' is not connected"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_chat_response_with_an_attachment_round_trips_through_json() {
        let msg = ServerMessage::ChatResponse {
            content: "here you go".into(),
            usage: None,
            attachments: vec![Attachment { mime_type: "image/png".into(), data: "aGVsbG8=".into() }],
            conversation_id: Some("c1".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"chatResponse","content":"here you go","usage":null,"attachments":[{"mimeType":"image/png","data":"aGVsbG8="}],"conversationId":"c1"}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_chat_response_without_an_attachments_field_defaults_to_empty() {
        // A peer from before this field existed (an older client build, or a stored fixture)
        // never sends `attachments` at all — must still parse, not error.
        let json = r#"{"type":"chatResponse","content":"hi","usage":null}"#;
        let msg = serde_json::from_str::<ServerMessage>(json).unwrap();
        assert!(matches!(msg, ServerMessage::ChatResponse { attachments, .. } if attachments.is_empty()));
    }

    #[test]
    fn client_skill_messages_round_trip_through_json() {
        let list = ClientMessage::ListSkills { request_id: 1 };
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(json, r#"{"type":"listSkills","requestId":1}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), list);

        let save = ClientMessage::SaveSkill {
            request_id: 2,
            skill: SkillDto { name: "review-pr".into(), description: "d".into(), body: "b".into(), agents: vec!["writer".into()] },
            overwrite: true,
        };
        let json = serde_json::to_string(&save).unwrap();
        assert_eq!(
            json,
            r#"{"type":"saveSkill","requestId":2,"skill":{"name":"review-pr","description":"d","body":"b","agents":["writer"]},"overwrite":true}"#
        );
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), save);

        let delete = ClientMessage::DeleteSkill { request_id: 3, name: "review-pr".into() };
        let json = serde_json::to_string(&delete).unwrap();
        assert_eq!(json, r#"{"type":"deleteSkill","requestId":3,"name":"review-pr"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), delete);
    }

    #[test]
    fn save_skill_without_an_agents_field_defaults_to_empty() {
        let json = r#"{"type":"saveSkill","requestId":2,"skill":{"name":"x","description":"d","body":"b"},"overwrite":false}"#;
        let msg = serde_json::from_str::<ClientMessage>(json).unwrap();
        assert!(matches!(msg, ClientMessage::SaveSkill { skill, .. } if skill.agents.is_empty()));
    }

    #[test]
    fn server_skill_messages_round_trip_through_json() {
        let list = ServerMessage::SkillList {
            request_id: 1,
            skills: vec![SkillDto { name: "x".into(), description: "d".into(), body: "b".into(), agents: Vec::new() }],
        };
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(
            json,
            r#"{"type":"skillList","requestId":1,"skills":[{"name":"x","description":"d","body":"b","agents":[]}]}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), list);

        let ok = ServerMessage::SkillOk { request_id: 2 };
        let json = serde_json::to_string(&ok).unwrap();
        assert_eq!(json, r#"{"type":"skillOk","requestId":2}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), ok);

        let err = ServerMessage::SkillError { request_id: 3, message: "no skill named 'x'".into() };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"type":"skillError","requestId":3,"message":"no skill named 'x'"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), err);
    }

    #[test]
    fn history_messages_round_trip_through_json() {
        let request = ClientMessage::RequestHistory { request_id: 1, limit: Some(50), conversation_id: None };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, r#"{"type":"requestHistory","requestId":1,"limit":50}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), request);

        let reply = ServerMessage::History {
            request_id: 1,
            messages: vec![HistoryMessage { role: HistoryRole::User, content: "hi".into(), created_at: 7, attachments: Vec::new() }],
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            json,
            r#"{"type":"history","requestId":1,"messages":[{"role":"user","content":"hi","createdAt":7,"attachments":[]}]}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), reply);

        let err = ServerMessage::HistoryError { request_id: 1, message: "boom".into() };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"type":"historyError","requestId":1,"message":"boom"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), err);
    }

    #[test]
    fn request_history_without_a_limit_field_means_everything() {
        let msg = serde_json::from_str::<ClientMessage>(r#"{"type":"requestHistory","requestId":2}"#).unwrap();
        assert_eq!(msg, ClientMessage::RequestHistory { request_id: 2, limit: None, conversation_id: None });
    }

    #[test]
    fn a_chat_from_before_conversations_existed_has_no_conversation_id() {
        let msg = serde_json::from_str::<ClientMessage>(r#"{"type":"chat","message":"hi"}"#).unwrap();
        assert_eq!(msg, ClientMessage::chat("hi"));
        assert_eq!(serde_json::to_string(&msg).unwrap(), r#"{"type":"chat","message":"hi"}"#);

        let with = ClientMessage::Chat { message: "hi".into(), conversation_id: Some("c1".into()), attachments: Vec::new() };
        let json = serde_json::to_string(&with).unwrap();
        assert_eq!(json, r#"{"type":"chat","message":"hi","conversationId":"c1"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), with);
    }

    #[test]
    fn a_chat_with_attachments_and_transcription_messages_round_trip() {
        let chat = ClientMessage::Chat {
            message: String::new(),
            conversation_id: None,
            attachments: vec![Attachment { mime_type: "application/pdf".into(), data: "JVBE".into() }],
        };
        let json = serde_json::to_string(&chat).unwrap();
        assert_eq!(json, r#"{"type":"chat","message":"","attachments":[{"mimeType":"application/pdf","data":"JVBE"}]}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), chat);

        let transcribe = ClientMessage::Transcribe { request_id: 1, audio: Attachment { mime_type: "audio/webm".into(), data: "GkXf".into() } };
        let json = serde_json::to_string(&transcribe).unwrap();
        assert_eq!(json, r#"{"type":"transcribe","requestId":1,"audio":{"mimeType":"audio/webm","data":"GkXf"}}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), transcribe);

        let text = ServerMessage::Transcription { request_id: 1, text: "hello".into() };
        assert_eq!(serde_json::to_string(&text).unwrap(), r#"{"type":"transcription","requestId":1,"text":"hello"}"#);
        let err = ServerMessage::TranscriptionError { request_id: 2, message: "no key".into() };
        assert_eq!(serde_json::to_string(&err).unwrap(), r#"{"type":"transcriptionError","requestId":2,"message":"no key"}"#);
    }

    #[test]
    fn conversation_messages_round_trip_through_json() {
        let cases: Vec<(ClientMessage, &str)> = vec![
            (ClientMessage::ListConversations { request_id: 1 }, r#"{"type":"listConversations","requestId":1}"#),
            (
                ClientMessage::RenameConversation { request_id: 2, conversation_id: "c1".into(), title: "Trip".into() },
                r#"{"type":"renameConversation","requestId":2,"conversationId":"c1","title":"Trip"}"#,
            ),
            (
                ClientMessage::DeleteConversation { request_id: 3, conversation_id: "c1".into() },
                r#"{"type":"deleteConversation","requestId":3,"conversationId":"c1"}"#,
            ),
        ];
        for (msg, expected) in cases {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
        }

        let replies: Vec<(ServerMessage, &str)> = vec![
            (
                ServerMessage::ConversationList {
                    request_id: 1,
                    conversations: vec![ConversationSummary { id: "c1".into(), title: "Trip".into(), created_at: 1, updated_at: 2 }],
                },
                r#"{"type":"conversationList","requestId":1,"conversations":[{"id":"c1","title":"Trip","createdAt":1,"updatedAt":2}]}"#,
            ),
            (ServerMessage::ConversationOk { request_id: 2 }, r#"{"type":"conversationOk","requestId":2}"#),
            (
                ServerMessage::ConversationError { request_id: 3, message: "boom".into() },
                r#"{"type":"conversationError","requestId":3,"message":"boom"}"#,
            ),
        ];
        for (msg, expected) in replies {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
        }
    }

    #[test]
    fn vault_messages_round_trip_through_json() {
        let requests: Vec<(ClientMessage, &str)> = vec![
            (ClientMessage::ListVaultFiles { request_id: 1 }, r#"{"type":"listVaultFiles","requestId":1}"#),
            (ClientMessage::ReadVaultNote { request_id: 2, path: "a.md".into() }, r#"{"type":"readVaultNote","requestId":2,"path":"a.md"}"#),
            (
                ClientMessage::SaveVaultNote { request_id: 3, path: "a.md".into(), content: "x".into(), expected_version: None },
                r#"{"type":"saveVaultNote","requestId":3,"path":"a.md","content":"x"}"#,
            ),
            (
                ClientMessage::SaveVaultNote { request_id: 3, path: "a.md".into(), content: "x".into(), expected_version: Some("v1".into()) },
                r#"{"type":"saveVaultNote","requestId":3,"path":"a.md","content":"x","expectedVersion":"v1"}"#,
            ),
            (
                ClientMessage::DeleteVaultNote { request_id: 4, path: "a.md".into(), expected_version: "v1".into() },
                r#"{"type":"deleteVaultNote","requestId":4,"path":"a.md","expectedVersion":"v1"}"#,
            ),
            (ClientMessage::SearchVault { request_id: 5, query: "milk".into() }, r#"{"type":"searchVault","requestId":5,"query":"milk"}"#),
        ];
        for (msg, expected) in requests {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
        }

        let replies: Vec<(ServerMessage, &str)> = vec![
            (ServerMessage::VaultFileList { request_id: 1, files: vec!["a.md".into()] }, r#"{"type":"vaultFileList","requestId":1,"files":["a.md"]}"#),
            (
                ServerMessage::VaultNote { request_id: 2, path: "a.md".into(), content: "x".into(), version: "v1".into() },
                r#"{"type":"vaultNote","requestId":2,"path":"a.md","content":"x","version":"v1"}"#,
            ),
            (ServerMessage::VaultSaved { request_id: 3, version: "v2".into() }, r#"{"type":"vaultSaved","requestId":3,"version":"v2"}"#),
            (ServerMessage::VaultOk { request_id: 4 }, r#"{"type":"vaultOk","requestId":4}"#),
            (
                ServerMessage::VaultSearchResults {
                    request_id: 5,
                    hits: vec![VaultSearchHit { path: "a.md".into(), line_number: 3, line: "buy milk".into() }],
                },
                r#"{"type":"vaultSearchResults","requestId":5,"hits":[{"path":"a.md","lineNumber":3,"line":"buy milk"}]}"#,
            ),
            (
                ServerMessage::VaultError { request_id: 6, message: "changed".into(), conflict: true },
                r#"{"type":"vaultError","requestId":6,"message":"changed","conflict":true}"#,
            ),
        ];
        for (msg, expected) in replies {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
        }
    }

    #[test]
    fn usage_messages_round_trip_through_json() {
        let request = ClientMessage::RequestUsage { request_id: 1, tz_offset_minutes: -180 };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, r#"{"type":"requestUsage","requestId":1,"tzOffsetMinutes":-180}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), request);
        assert_eq!(
            serde_json::from_str::<ClientMessage>(r#"{"type":"requestUsage","requestId":1}"#).unwrap(),
            ClientMessage::RequestUsage { request_id: 1, tz_offset_minutes: 0 }
        );

        let extend = ClientMessage::ExtendLimit { request_id: 2, limit_id: "day".into() };
        let json = serde_json::to_string(&extend).unwrap();
        assert_eq!(json, r#"{"type":"extendLimit","requestId":2,"limitId":"day"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), extend);

        let limit = LimitStatusDto {
            id: "day".into(),
            scope: "global".into(),
            window_hours: 24,
            used_tokens: 10,
            max_tokens: Some(8),
            used_cost_usd: 0.5,
            max_cost_usd: None,
            fraction: 1.25,
            warn: true,
            exceeded: true,
            unpriced_calls: 1,
            frees_up_in_minutes: Some(30),
            extend_tokens: 2,
            extend_cost_usd: 0.0,
        };
        let report = ServerMessage::UsageReport {
            request_id: 1,
            report: UsageReportDto {
                total: Usage { prompt_tokens: 7, completion_tokens: 3, total_tokens: 10 },
                conversation_count: 1,
                message_count: 1,
                by_device: vec![DeviceUsage {
                    device_id: "web-1".into(),
                    name: Some("Browser".into()),
                    conversation_count: 1,
                    message_count: 1,
                    usage: Usage { prompt_tokens: 7, completion_tokens: 3, total_tokens: 10 },
                }],
                daily: vec![DailyUsageDto { date: "2026-09-25".into(), calls: 1, tokens: 10 }],
                limits_enabled: true,
                limits: vec![limit.clone()],
                recent: Some(RecentSpendDto {
                    window_hours: 24,
                    by_model: vec![SpendBucketDto { key: "m".into(), calls: 1, tokens: 10, cost_usd: 0.5, unpriced_calls: 0 }],
                    by_channel: Vec::new(),
                }),
                ledger_error: None,
            },
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.starts_with(r#"{"type":"usageReport","requestId":1,"report":{"total":{"#), "{json}");
        assert!(json.contains(r#""byDevice":[{"deviceId":"web-1","name":"Browser","conversationCount":1"#), "{json}");
        assert!(json.contains(r#""extendTokens":2"#) && json.contains(r#""limitsEnabled":true"#), "{json}");
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), report);

        for (msg, expected) in [
            (ServerMessage::UsageError { request_id: 3, message: "boom".into() }, r#"{"type":"usageError","requestId":3,"message":"boom"}"#),
            (
                ServerMessage::ChatError { message: "limit".into(), conversation_id: Some("c1".into()), spend_limit_id: Some("day".into()) },
                r#"{"type":"chatError","message":"limit","conversationId":"c1","spendLimitId":"day"}"#,
            ),
        ] {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(json, expected);
            assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
        }
        let extended = ServerMessage::LimitExtended { request_id: 2, limit };
        let json = serde_json::to_string(&extended).unwrap();
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), extended);
    }

    #[test]
    fn settings_messages_round_trip_through_json() {
        let update = HubSettingsUpdate {
            providers: vec![ProviderEditDto {
                original_id: Some("main".into()),
                id: "primary".into(),
                kind: "anthropic".into(),
                base_url: String::new(),
                model: String::new(),
                api_key: SecretEdit::Set("sk".into()),
            }],
            active_provider: "primary".into(),
            agents: Vec::new(),
            tavily_key: SecretEdit::Keep,
            whisper_key: SecretEdit::Clear,
            limits: None,
            prices: Vec::new(),
        };
        assert!(update.sets_a_secret());
        let save = ClientMessage::SaveSettings { request_id: 1, pairing_key: "k".into(), base_version: "v".into(), update };
        let json = serde_json::to_value(&save).unwrap();
        assert_eq!(json["type"], "saveSettings");
        assert_eq!(json["update"]["providers"][0]["originalId"], "main");
        assert_eq!(json["update"]["providers"][0]["apiKey"], serde_json::json!({ "action": "set", "value": "sk" }));
        assert_eq!(json["update"]["tavilyKey"], serde_json::json!({ "action": "keep" }));
        assert_eq!(json["update"]["whisperKey"], serde_json::json!({ "action": "clear" }));
        assert_eq!(serde_json::from_value::<ClientMessage>(json).unwrap(), save);

        let request: ClientMessage = serde_json::from_str(r#"{"type":"requestSettings","requestId":3}"#).unwrap();
        assert_eq!(request, ClientMessage::RequestSettings { request_id: 3 });

        let error: ServerMessage = serde_json::from_str(r#"{"type":"settingsError","requestId":3,"message":"m"}"#).unwrap();
        assert_eq!(error, ServerMessage::SettingsError { request_id: 3, message: "m".into(), conflict: false, auth_rejected: false });
        let status = SecretStatusDto { set: true, hint: None };
        assert_eq!(serde_json::to_string(&status).unwrap(), r#"{"set":true}"#);
    }

    #[test]
    fn device_messages_use_the_web_shapes() {
        let set: ClientMessage =
            serde_json::from_str(r#"{"type":"setDeviceStatus","requestId":4,"pairingKey":"k","deviceId":"phone","action":"revoke"}"#).unwrap();
        assert_eq!(set, ClientMessage::SetDeviceStatus { request_id: 4, pairing_key: "k".into(), device_id: "phone".into(), action: DeviceAction::Revoke });

        let list = ServerMessage::DeviceList {
            request_id: 4,
            devices: vec![DeviceDto { device_id: "phone".into(), device_name: "Phone".into(), status: DeviceStatusDto::Pending, first_seen_ms: 1, last_seen_ms: 2 }],
            you: "web-1".into(),
        };
        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(json["type"], "deviceList");
        assert_eq!(json["devices"][0], serde_json::json!({ "deviceId": "phone", "deviceName": "Phone", "status": "pending", "firstSeenMs": 1, "lastSeenMs": 2 }));

        let error: ServerMessage = serde_json::from_str(r#"{"type":"deviceError","requestId":4,"message":"m"}"#).unwrap();
        assert_eq!(error, ServerMessage::DeviceError { request_id: 4, message: "m".into(), auth_rejected: false });
    }

    #[test]
    fn client_discover_round_trips_through_json() {
        let msg = ClientMessage::Discover;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"discover"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_discover_ack_round_trips_through_json() {
        let msg = ServerMessage::DiscoverAck { server_name: "Fabio's Desktop".into(), secure_url: None };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"discoverAck","serverName":"Fabio's Desktop"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_discover_ack_carries_the_secure_url_only_when_there_is_one() {
        let msg = ServerMessage::DiscoverAck { server_name: "hub".into(), secure_url: Some("wss://hub.tail1234.ts.net:7420".into()) };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"discoverAck","serverName":"hub","secureUrl":"wss://hub.tail1234.ts.net:7420"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn server_tool_call_request_round_trips_through_json() {
        let msg = ServerMessage::ToolCallRequest {
            call_id: 3,
            tool: "read_file".into(),
            arguments: serde_json::json!({"path": "abc"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"toolCallRequest","callId":3,"tool":"read_file","arguments":{"path":"abc"}}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), msg);
    }
}
