use serde::{Deserialize, Serialize};
use serde_json::Value;
use warden_core::code_engine::{CodeEvent, ToolStatus};
use warden_core::model::{Attachment, Message, StreamEvent, Usage};
use warden_core::project::Project;
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
    /// P104: a suggestion the assistant made after a conversation, not yet accepted — invisible to the
    /// model until it's saved without this flag. Sent back as it came, so editing a suggestion doesn't accept it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub proposed: bool,
    /// The conversation a suggestion came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// When a suggestion was made, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_at: Option<i64>,
    /// P115: the suggestion is a change to this existing skill; accepting it applies it there. Only read by
    /// clients to say so — a save never sets it, the hub keeps what is on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revises: Option<String>,
}

impl From<Skill> for SkillDto {
    fn from(skill: Skill) -> Self {
        Self { name: skill.name, description: skill.description, body: skill.body, agents: skill.agents, proposed: skill.proposed, source: skill.source, proposed_at: skill.proposed_at, revises: skill.revises }
    }
}

/// A project (P103) on the wire — what a client's Projects screen lists and edits over `ListProjects`/`SaveProject`.
/// The files of a project are ordinary notes of the vault (`projects/<id>/…`) and travel over the vault messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    /// The folder name: letters, digits, `-` and `_`. Never changes; conversations point at it.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
    /// A code project's working folder on the hub's machine (P103 b); absent for an ordinary project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Its conversations are driven by a code engine in `workdir` (needs one).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub code: bool,
}

impl From<Project> for ProjectDto {
    fn from(project: Project) -> Self {
        Self { id: project.id, name: project.name, description: project.description, instructions: project.instructions, workdir: project.workdir, code: project.code }
    }
}

/// One thing a code engine does during a task (P103 b), as `ChatEvent` tells it while the task runs. The task's end is
/// still `ChatResponse`/`ChatError`; these only let the client show the work as it happens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChatEventDto {
    /// More of the answer's text.
    Text { text: String },
    /// A tool the engine is using. `call_id` is the same for each stage of one call, so a client updates one line.
    Tool { call_id: String, tool: String, title: String, status: ToolStatusDto },
    /// Something the engine said about itself that isn't the answer (a retry, a wait).
    Notice { text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolStatusDto {
    Running,
    Completed,
    Failed,
}

impl ChatEventDto {
    /// What a client is shown of an engine event: `None` for the ones that are the hub's business (the session).
    pub fn from_code(event: CodeEvent) -> Option<Self> {
        Some(match event {
            CodeEvent::Text(text) => Self::Text { text },
            CodeEvent::Notice(text) => Self::Notice { text },
            CodeEvent::Session(_) => return None,
            CodeEvent::Tool(tool) => Self::Tool {
                call_id: tool.call_id,
                tool: tool.tool,
                title: tool.title,
                status: match tool.status {
                    ToolStatus::Running => ToolStatusDto::Running,
                    ToolStatus::Completed => ToolStatusDto::Completed,
                    ToolStatus::Failed => ToolStatusDto::Failed,
                },
            },
        })
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
    /// The agent (P46) this conversation last spoke with, so a client restores its selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The project (P103) this conversation belongs to, so a client groups its list. Absent for a conversation
    /// outside any project, and for a hub from before projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The folder of the hub's machine (P102) this conversation works in. Absent for a conversation in a project
    /// (it has the project's), for one with no folder, and for a hub from before folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

/// A folder in a `DirList` (P102): its name for the list and its whole path to choose or open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
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

/// Dollars spent on one day of a `UsageReport`, from the spend ledger (P10); `date` is `YYYY-MM-DD` in the
/// viewer's time zone. The ledger only keeps `RecentSpendDto.window_hours`, so an older day reads zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyCostDto {
    pub date: String,
    pub calls: u64,
    pub cost_usd: f64,
    pub unpriced_calls: u64,
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
            by_provider: b.by_provider.into_iter().map(Into::into).collect(),
            by_agent: b.by_agent.into_iter().map(Into::into).collect(),
            by_person: b.by_person.into_iter().map(Into::into).collect(),
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
    /// The `[[providers]]` id that answered (P10); an empty key is a call from before it was kept.
    #[serde(default)]
    pub by_provider: Vec<SpendBucketDto>,
    /// The agent the turn started with; an empty key is a turn with no agent chosen.
    #[serde(default)]
    pub by_agent: Vec<SpendBucketDto>,
    /// The workspace member (P84); an empty key is the owner.
    #[serde(default)]
    pub by_person: Vec<SpendBucketDto>,
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
    /// Dollars per day from the spend ledger (P10); empty with spending limits off, which keeps no ledger.
    #[serde(default)]
    pub daily_cost: Vec<DailyCostDto>,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// P84: the member it belongs to; absent for the owner's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// What a node offers (P93), as it announced itself in `Hello`. Its operator chose it on the node
/// (`--shell`, `--files`); the hub still decides who may use it (`[[nodes]]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NodeOfferDto {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Runs shell commands.
    #[serde(default)]
    pub shell: bool,
    /// Reads and writes files in one folder.
    #[serde(default)]
    pub files: bool,
    /// Tools of the MCP servers this node lends (fatia 2), each with its own schema, as the node names
    /// them. The hub offers each as a tool of its own, `<node>__<tool>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<ToolSpec>,
    /// Model providers this node lends (fatia 3), by their id on the node. A hub reaches one through
    /// a `[[providers]]` entry with `kind = "node"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

/// One node for the screens (P93): a device that announced itself as a node, or one `[[nodes]]`
/// names, with what it offers and what the hub lets agents do with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfoDto {
    pub device_id: String,
    pub name: String,
    pub online: bool,
    /// Approved in the hub's device list — needed before any agent can use it.
    pub approved: bool,
    /// What it offered the last time it connected to this hub; absent when it hasn't since the hub started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<NodeOfferDto>,
    pub enabled: bool,
    /// Agents allowed to use it; empty = every agent.
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub require_approval: bool,
}

/// One Warden API key (P12), for the settings screens. Never the key or its hash: `shown` is its
/// first characters, for recognizing it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyDto {
    pub id: String,
    pub name: String,
    pub shown: String,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<i64>,
    /// The only agent this key speaks as; absent for a general key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// P84: the member the key belongs to (calls with it run as them); absent for the owner's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// One scheduled task (P92), as `[[tasks]]` keeps it. Exactly one of `every`, `cron` and `once`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDto {
    pub id: String,
    /// The agent that runs it; absent runs with no persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    pub enabled: bool,
}

/// One incoming webhook (P105), as `[[webhooks]]` keeps it. The credential isn't part of it: it lives in its own file and
/// is made by `CreateWebhookCredential`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookDto {
    pub id: String,
    /// The agent that runs it; absent runs with no persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub prompt: String,
    pub enabled: bool,
    /// How the caller proves itself: `"token"` (a bearer token) or `"hmac"` (a signature of the body, GitHub or Stripe
    /// style). Absent is a token.
    #[serde(default = "default_webhook_auth")]
    pub auth: String,
}

fn default_webhook_auth() -> String {
    "token".to_string()
}

/// A webhook and what the hub knows about its credential, for the list on the screens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookInfoDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub prompt: String,
    pub enabled: bool,
    /// What the webhook wants: `"token"` or `"hmac"`.
    pub auth: String,
    /// What it has: `"token"`, `"hmac"` or absent (no credential, so it takes no calls). A webhook whose credential is
    /// of the other kind than `auth` takes no calls either, until a new one is made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// The first characters of the credential, for recognizing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<i64>,
    /// The id of its conversation, `task-hook-<id>`, which every device lists.
    pub conversation: String,
}

/// A folder of the owner's vault shared with members (P84 fatia 3). A member sees it at
/// `compartilhado/<id>/` in their own vault.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceDto {
    pub id: String,
    pub folder: String,
    /// Usernames, or `"*"` for everyone.
    #[serde(default)]
    pub readers: Vec<String>,
    /// Writers also read.
    #[serde(default)]
    pub writers: Vec<String>,
}

/// A member of the workspace (P84), as the hub shows them — never the password hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfoDto {
    /// The username.
    pub id: String,
    pub name: String,
    /// `member` today.
    pub role: String,
    /// Still on the provisional password the root set: the hub only lets them change it.
    #[serde(default)]
    pub must_change_password: bool,
    /// P84 fatia 2: the tools the owner set for them; `None` is the safe default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Their own agents' ids — the owner sees that they exist, not what they say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    /// P84 fatia 4: their data is encrypted on this hub, with a key only they (and their recovery
    /// code) can open.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub encrypted: bool,
    /// The owner reset their password: the data opens only with the recovery code, given when they
    /// choose the new password.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_recovery: bool,
    /// Only in `HelloAck`: the hub doesn't hold their key (it restarted since they last signed in
    /// with the password), so their data can't be opened until they do.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// P84 fatia 4 parte B: who besides them may open their data — `private`, `consent` or `company`;
    /// empty while it isn't encrypted. What their data follows now, which is the workspace's policy
    /// unless a change to a weaker one still waits for their yes (`policy_pending`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub member_policy: String,
    /// The workspace's policy has changed to a weaker one and they haven't accepted it yet — or a
    /// client that can show the new recovery code it comes with hasn't signed in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub policy_pending: bool,
    /// The workspace's recovery policy — only in `HelloAck`, for the member to read.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recovery_policy: String,
    /// P115: the member turned off the assistant learning from their conversations.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub learning_opt_out: bool,
    /// P115: the model the owner chose for learning from their conversations (`None`: the workspace's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_provider: Option<String>,
    /// The workspace has learning on at all — only in `HelloAck`, so the member's switch means something.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub learning_enabled: bool,
    /// Every time the owner recovered their data with the workspace's recovery key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recoveries: Vec<RecoveryEventDto>,
    /// P84 fatia 5: the TruthID username they linked, empty if none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub truthid: String,
    /// An invite to link a TruthID is open (made by the owner, not used or expired yet).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invite_open: bool,
    /// P102: the folders of the hub's machine the owner allowed them to work in (absolute paths).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdirs: Vec<String>,
    /// P102 fatia 2: the same on nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_workdirs: Vec<NodeFolderDto>,
}

/// A folder on a node a member may work in (P102 fatia 2): `path` is relative to the folder the node lends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeFolderDto {
    pub node: String,
    #[serde(default)]
    pub path: String,
}

/// A member the owner removed whose encrypted data is still on disk, with the key that opens it (P84 fatia 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedUserDto {
    pub id: String,
    pub name: String,
}

/// The owner recovered someone's data with the workspace's recovery key (P84 fatia 4, parte B).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryEventDto {
    /// Milliseconds since the epoch.
    pub at_ms: i64,
    /// The policy it was done under: `consent` or `company`.
    pub kind: String,
    /// The person has seen it.
    #[serde(default)]
    pub seen: bool,
}

/// A task and where it stands on this hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfoDto {
    #[serde(flatten)]
    pub task: TaskDto,
    /// Absent when paused, done (a `once` that ran) or the schedule is invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_finished_at_ms: Option<i64>,
    /// Why the last finished run failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_error: Option<String>,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "camelCase")]
pub enum SecretEdit {
    #[default]
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
    /// Kind "node" only (P93): the node's device id. Empty otherwise.
    #[serde(default)]
    pub node: String,
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
    /// Kind "node" only (P93): the node's device id.
    #[serde(default)]
    pub node: String,
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
    /// P46 "funcionários" mode. `#[serde(default)]`: a screen from before it existed leaves it off.
    #[serde(default)]
    pub can_message_agents: bool,
    /// P92: the `manage_tasks` tool. `#[serde(default)]` for the same reason.
    #[serde(default)]
    pub can_manage_tasks: bool,
    /// `None` keeps every tool.
    pub allowed_tools: Option<Vec<String>>,
    /// P122: 1 only answers, 2 suggests, 3 asks before every change, 4 acts on its own. `#[serde(default)]`: a screen
    /// from before it existed leaves the agent at 4, as it was.
    #[serde(default = "default_autonomy")]
    pub autonomy: u8,
    /// P122: the kinds of action that need a person's yes even at autonomy 4, by id (`delete_data`, `spend_money`,
    /// `critical_infra`, `external_message`, `publish_code`, `important_config`, `elevated_agent`). Empty: none.
    #[serde(default)]
    pub approval_required: Vec<String>,
    /// P120: the agent's role in the organization (free text) and the id of the agent it reports to. Only shown for
    /// now: they change nothing an agent may do. Absent on a member's agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reports_to: Option<String>,
    /// P84: members this agent is shared with, by username, or `"*"` for everyone. Empty: the owner's alone.
    #[serde(default)]
    pub shared_with: Vec<String>,
    /// P84: in a member's view, their username on their own agents; absent on the ones shared with
    /// them (and on every agent in the owner's view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// P123: the models this agent may pick for the tasks it delegates (provider, combo or policy ids); the first is the default of a
    /// delegation that names none. Empty leaves the choice open. A member's agent has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delegation_models: Vec<String>,
}

fn default_autonomy() -> u8 {
    4
}

/// One task an agent delegated in the background (P123), as the screens list it. `state` is `pending`, `running`, `waiting` (its
/// agent waits for a subtask), `paused` (a person paused it), `done`, `failed` or `cancelled`; `group` is shared by the tasks one turn started, which is what a progress is counted over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskDto {
    pub id: String,
    pub group: String,
    /// The agent that delegated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The agent that does the work, or the name given to a temporary helper.
    pub assignee: String,
    /// The task this one is a subtask of (P123), when its agent started it from inside another task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub objective: String,
    /// The provider or combo chosen for this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub channel: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
    /// This task is running in the process that answered, so it can be paused, resumed or stopped from a screen
    /// (`ControlAgentTask`). A task of another process or one that has finished isn't.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub controllable: bool,
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

/// One named combo (P90): provider ids, tried in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComboDto {
    pub id: String,
    pub providers: Vec<String>,
}

/// One named model policy (P123): `id` ("fast", "reasoning"...) is answered by `model`, a provider or combo; `description` tells an agent
/// that delegates when to pick it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPolicyDto {
    pub id: String,
    pub model: String,
    #[serde(default)]
    pub description: String,
}

/// One provider switch in a turn (P79): `from` failed with `reason`, `to` answered with `model`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFallbackDto {
    pub from: String,
    pub to: String,
    pub model: String,
    pub reason: String,
}

impl From<warden_core::model::ProviderFallback> for ProviderFallbackDto {
    fn from(f: warden_core::model::ProviderFallback) -> Self {
        Self { from: f.from, to: f.to, model: f.model, reason: f.reason }
    }
}

/// `[git_sync]` as the settings screen shows it (P61): the remote's URL (empty = no git sync) and
/// whether a token is saved, never the token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSyncSettingsDto {
    pub remote_url: String,
    pub token: SecretStatusDto,
}

/// `[git_sync]` as a save sends it. An empty `remote_url` turns git sync off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSyncEditDto {
    pub remote_url: String,
    pub token: SecretEdit,
}

/// Where the hub's vault syncs to (P61): nowhere until it has a vault key, then git when
/// `[git_sync]` is set, Arweave otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncBackendDto {
    NotSetUp,
    Git,
    Arweave,
}

/// One sync round the hub ran (its 5-minute loop or a "sync now"). `pulled`/`pushed` are only
/// there when something moved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRoundDto {
    pub at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulled: Option<SyncPulledDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed: Option<SyncPushedDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPulledDto {
    pub files_written: usize,
    pub files_deleted: usize,
    pub config_updated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPushedDto {
    pub commit_sha: String,
    pub files_changed: usize,
}

/// The hub's sync state for the web's Sync screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusDto {
    pub backend: SyncBackendDto,
    /// `[git_sync]`'s URL, never its token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at_ms: Option<i64>,
    pub pending_vault_changes: usize,
    pub pending_config_changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_round: Option<SyncRoundDto>,
    /// Until when the hub is showing a pairing code (P88). The code only goes back in the reply
    /// to the `PairHost` that asked for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosting_until_ms: Option<i64>,
    /// How the last pairing the hub showed a code for ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pairing: Option<SyncPairingDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPairingDto {
    pub at_ms: i64,
    /// Absent: a device joined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What `SyncAction` asks the hub to do — the same as `warden-server sync now|init|pair`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SyncActionDto {
    /// Runs a round now instead of waiting for the loop.
    SyncNow,
    /// Makes the hub the first device of a sync group, with a fresh vault key.
    Init,
    /// Receives the vault key from a device showing `code`. `host` (an IPv4 address) is the only
    /// place tried when given — how a hub reaches a device over Tailscale; otherwise the hub's LAN
    /// is swept.
    PairJoin {
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
    /// Shows a pairing code other devices join through (P88), answered with the code in
    /// `SyncStatus::pairing_code`. Asking again while one is up hands back the same code.
    PairHost,
    /// Stops showing the code.
    CancelPairHost,
}

/// The part of the hub's `config.toml` the web settings screen shows (P78): providers, agents, the
/// Tavily/Whisper keys, spending limits, prices, the git sync remote (P61), the bots, the Telegram
/// token and the delegation/TruthID settings (P119). Shell, MCP servers, SSH hosts, storage paths and
/// the embedded hub (`machine`) would let a paired device run commands on the hub's machine, so they
/// only change when the hub was started with `--allow-machine-settings`, and only over an encrypted
/// or local connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubSettingsDto {
    pub providers: Vec<ProviderSettingsDto>,
    /// Empty when none is picked.
    pub active_provider: String,
    /// Named routing combos (P90): each one's providers are tried in order when one is down.
    /// A combo id can be the active model or an agent's default, like a provider id.
    #[serde(default)]
    pub combos: Vec<ComboDto>,
    /// Named model policies (P123): names an agent that delegates may use for a task's model, each answered by a provider or combo.
    #[serde(default)]
    pub model_policies: Vec<ModelPolicyDto>,
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
    pub git_sync: GitSyncSettingsDto,
    /// Learning and the bots' allow-lists (P118).
    #[serde(default)]
    pub bots: BotsSettingsDto,
    /// The Telegram bot's token (P119): whether one is set, never the token.
    #[serde(default)]
    pub telegram_token: SecretStatusDto,
    /// Delegation ceilings and TruthID (P119).
    #[serde(default)]
    pub advanced: AdvancedSettingsDto,
    /// What reaches the hub's own machine (P119): see [`MachineSettingsDto`].
    #[serde(default)]
    pub machine: MachineSettingsDto,
    /// Things outside the file that change what it means on this hub (a `--provider` flag, a
    /// providers list still empty, ...), one sentence each.
    pub notes: Vec<String>,
}

/// A stranger waiting for the owner to approve their access to a bot (P117).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotPairingDto {
    /// `telegram` or `whatsapp`.
    pub channel: String,
    /// The Telegram user id or the WhatsApp chat id that would go on the allow-list.
    pub sender: String,
    /// The name they go by, possibly empty.
    pub label: String,
    /// What the sender was told, as `ABCD-EFGH`.
    pub code: String,
    /// Unix seconds.
    pub expires_at: u64,
}

/// A member of the workspace a chat may speak as when its pairing is approved (P117): whether the bots are
/// linked to the hub as them (`warden bots link`), without which approving a chat as them is refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotMemberDto {
    pub id: String,
    pub name: String,
    pub linked: bool,
}

/// `[learning]`, `[telegram] allowed_users` and `[whatsapp] allowed_chats` (P118), shown and saved
/// as one block. The Telegram token isn't here: it's a secret, so it travels as `telegram_token`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotsSettingsDto {
    pub learning_enabled: bool,
    /// A provider or combo id for the learning calls; empty means the active model.
    pub learning_provider: String,
    pub learning_max_per_day: u32,
    /// `telegram:<chat id>` / `whatsapp:<chat id>`: the bot chats the assistant may learn from.
    pub learning_bot_chats: Vec<String>,
    /// Telegram user ids (numbers) that may talk to the bot. Empty means nobody.
    pub telegram_allowed_users: Vec<i64>,
    /// WhatsApp numbers or ids that may talk to the bot. Empty means nobody.
    pub whatsapp_allowed_chats: Vec<String>,
    /// A stranger who writes to the Telegram bot gets a code for the owner to approve (P117).
    #[serde(default)]
    pub telegram_pairing: bool,
    /// The same for the WhatsApp bot.
    #[serde(default)]
    pub whatsapp_pairing: bool,
}

/// Delegation ceilings and TruthID (P119), shown and saved as one block. None is a secret and none
/// reaches the machine, so it needs no special connection; a ceiling only counts when it changes
/// (see `apply_advanced`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedSettingsDto {
    /// How deep a sub-agent may itself delegate. `None` keeps the built-in depth.
    pub delegate_max_depth: Option<u32>,
    /// Model calls the sub-agents may make between them in one turn. `None` keeps the built-in
    /// ceiling; `0` (switching it off) is only set by hand in the file.
    pub max_delegated_calls: Option<u32>,
    /// Background jobs one turn may run at the same time. `None` keeps the built-in number.
    pub max_parallel_jobs: Option<u32>,
    /// `base-mainnet` or `base-sepolia`.
    pub truthid_network: String,
    /// Empty uses the network's public endpoint.
    pub truthid_rpc_url: String,
    /// The hub's own `https://` address, what a TruthID login is sent to. Empty turns that login off.
    pub truthid_public_url: String,
}

/// One external MCP server as the screen shows it: the names of its secret values (env entries or
/// headers), never the values (P119).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerSettingsDto {
    pub name: String,
    /// `stdio` (a local process) or `http`.
    pub kind: String,
    pub command: String,
    pub args: Vec<String>,
    /// The names of the process's environment entries.
    pub env_keys: Vec<String>,
    pub url: String,
    /// The names of the request headers.
    pub header_keys: Vec<String>,
    /// Signs in through the OAuth flow, which only the desktop can run.
    pub oauth: bool,
}

/// One env entry or header of an MCP server being saved: `Keep` carries the saved value over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretEntryEdit {
    pub key: String,
    pub value: SecretEdit,
}

/// One MCP server being saved. `original_name` finds the saved one, so `Keep` can carry its values
/// (and its `oauth` choice) over to a renamed server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerEditDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
    pub name: String,
    /// `stdio` or `http`.
    pub kind: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<SecretEntryEdit>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<SecretEntryEdit>,
}

/// One SSH server the AI may run commands on (P47), shown and saved as is: only the path to a key
/// lives here, never the key. `port` is wide so an out-of-range value reaches the check and gets a
/// readable refusal instead of a parse error.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHostDto {
    pub id: String,
    pub host: String,
    pub user: String,
    pub port: u32,
    pub identity_file: String,
    pub enabled: bool,
    pub agents: Vec<String>,
    pub require_approval: bool,
}

/// The desktop's embedded hub (`[embedded_server]`) as the web edits it. Its `auth_key` is not here:
/// it is the pairing key this very screen signs in with, and changing it over the same channel would
/// lock the owner out, so it only changes on the desktop. Nothing here reloads a running listener:
/// it counts from the hub's next start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedServerDto {
    /// Starts with the desktop.
    pub enabled: bool,
    pub port: u16,
    /// Empty listens on every interface.
    pub listen_host: String,
    pub server_name: String,
    pub tailscale_cert: bool,
    pub tls_cert: String,
    pub tls_key: String,
    pub tls_host: String,
    pub web_ui: bool,
}

/// What reaches the hub's own machine (P119): the shell tool, where the vault and generated files
/// live, the MCP servers it starts, the SSH hosts it may reach and the embedded hub. The hub only lets
/// a save change any of it when it was started with `--allow-machine-settings` and the connection is
/// encrypted or local; otherwise `writable` is false and `blocked_reason` says why.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineSettingsDto {
    pub writable: bool,
    /// Why `writable` is false, one sentence; empty when it is true.
    pub blocked_reason: String,
    pub enable_shell: bool,
    /// Empty uses the hub's default.
    pub vault_path: String,
    /// Empty uses the default next to the vault.
    pub generated_path: String,
    pub mcp_servers: Vec<McpServerSettingsDto>,
    pub ssh_hosts: Vec<SshHostDto>,
    /// `None` when the hub has no `[embedded_server]`: that one is set up on the desktop.
    pub embedded_server: Option<EmbeddedServerDto>,
}

/// The machine settings being saved; see [`MachineSettingsDto`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineEditDto {
    pub enable_shell: bool,
    pub vault_path: String,
    pub generated_path: String,
    pub mcp_servers: Vec<McpServerEditDto>,
    pub ssh_hosts: Vec<SshHostDto>,
    /// `None` leaves `[embedded_server]` as it is; `Some` changes an existing one (the web never creates it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded_server: Option<EmbeddedServerDto>,
}

impl MachineEditDto {
    /// Whether this save carries a new secret value for an MCP server's env or headers.
    pub fn sets_a_secret(&self) -> bool {
        self.mcp_servers.iter().any(|s| s.env.iter().chain(&s.headers).any(|e| e.value.is_set()))
    }
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
    /// `None` (or absent) leaves `[git_sync]` as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sync: Option<GitSyncEditDto>,
    /// `None` (or absent) keeps the combos, dropping any provider this save removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combos: Option<Vec<ComboDto>>,
    /// `None` (or absent) keeps the model policies (P123), dropping any whose model this save removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policies: Option<Vec<ModelPolicyDto>>,
    /// `None` (or absent) leaves `[learning]` and the bots' lists as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bots: Option<BotsSettingsDto>,
    /// What to do with the Telegram bot's token (P119). Absent keeps it.
    #[serde(default)]
    pub telegram_token: SecretEdit,
    /// `None` (or absent) leaves the delegation ceilings and TruthID as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advanced: Option<Box<AdvancedSettingsDto>>,
    /// `None` (or absent) leaves everything that reaches the machine as it is. A hub started without
    /// `--allow-machine-settings`, or a connection that isn't encrypted or local, refuses a save that has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<Box<MachineEditDto>>,
}

impl HubSettingsUpdate {
    /// Whether this save carries a new secret, which the hub only accepts over an encrypted or local
    /// connection.
    pub fn sets_a_secret(&self) -> bool {
        self.tavily_key.is_set()
            || self.whisper_key.is_set()
            || self.telegram_token.is_set()
            || self.providers.iter().any(|p| p.api_key.is_set())
            || self.git_sync.as_ref().is_some_and(|g| g.token.is_set())
            || self.machine.as_ref().is_some_and(|m| m.sets_a_secret())
    }
}

/// One change a person makes to the organization of the owner's agents from the tree (P120), without touching anything else of the
/// settings. `role` and `reports_to` left out (or blank) mean none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AgentOrgEdit {
    /// Gives `id` this role and this superior (moving it, with everyone below it, when the superior changes).
    SetPosition {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reports_to: Option<String>,
    },
    /// A new agent under `reports_to` (or at the top), careful by default: read-only tools, asks before every change, can't delegate
    /// or manage agents until a person turns that on.
    AddReport {
        id: String,
        persona: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reports_to: Option<String>,
    },
    /// Removes `id`; whoever reported to it reports to its superior.
    Remove { id: String },
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
        /// P93: this connection is a node (`warden-server node`) lending what it has to the hub's
        /// agents. Absent for every other client.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<NodeOfferDto>,
        /// P84: pairs as this member (with `password`) instead of with the pairing key. Only needed
        /// until the hub issues a token, like the key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<String>,
        /// P84 fatia 4: this client can show a member's recovery code, once, and make them keep it.
        /// The hub only turns encryption on for someone's data from a client that says so: a code
        /// nobody sees is a key nobody has.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery_codes: bool,
        /// P84 fatia 5: asks to sign in as a member with their TruthID instead of a password. The hub
        /// answers `TruthIdChallenge` (a QR for the TruthID app) and, once the phone approves, `HelloAck`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truthid_login: bool,
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
        /// The configured agent (P46) this turn speaks as: its persona, model, skills and tool
        /// list, plus the tools its flags give it. Saved on the conversation. `None` = no agent,
        /// what every client from before this field sends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        /// The project (P103) a conversation **starts** in: it is saved on the conversation when this turn
        /// creates it and runs in the project's folder, with its instructions. Ignored for a conversation
        /// that already exists, which stays where it was created. A project the hub doesn't have is a `ChatError`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
        /// The folder of the hub's machine (P102) a conversation in no project **starts** in, with the same rule as
        /// `project_id`: saved when this turn creates the conversation, ignored for one that exists or that is in a
        /// project. The hub checks that the person may use it (a member only inside the folders the owner allowed
        /// them) and answers a `ChatError` otherwise.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
    },
    /// The person's answer to a `ServerMessage::ApprovalRequest` (P46 `manage_agents`, SSH hosts
    /// that need approval). An id with no pending request is ignored.
    ResolveApproval {
        approval_id: u64,
        approved: bool,
        /// "Yes, and the same kind of ask from now on" (P103 b) — only meaningful on an approval that offered it
        /// (`ApprovalRequest.always`), and then `approved` is true too.
        #[serde(default)]
        always: bool,
    },
    /// Stops the task a code project's conversation is running (P103 b). The work done so far is kept, and the turn
    /// ends with the usual `ChatResponse`. Harmless for a conversation that isn't running one.
    CancelTurn {
        conversation_id: String,
    },
    /// How much a code conversation asks before acting (P103 b): `manual`, `acceptEdits`, `acceptAll` or `plan`
    /// (anything else is `manual`). Takes effect at once, a task that is running included; the hub forgets it on a
    /// restart, so a client says it again with each `Chat`. Ignored for a member.
    SetCodeMode {
        conversation_id: String,
        mode: String,
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
    /// A node's answer to `ServerMessage::ModelRequest` (P93), one stream event at a time.
    ModelEvent {
        request_id: u64,
        event: StreamEvent,
    },
    /// The node's model finished that answer.
    ModelDone {
        request_id: u64,
    },
    /// The node's model failed. `transient`: it was busy or unreachable there (a 503, Ollama down),
    /// so a hub combo may move on to its next provider.
    ModelError {
        request_id: u64,
        message: String,
        #[serde(default)]
        transient: bool,
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
    /// Projects (P103) — the projects of the person's own vault. Answered by `ProjectList`, `ProjectOk` or `ProjectError`
    /// with the same `request_id`.
    ListProjects {
        request_id: u64,
    },
    /// Creates (`overwrite: false`, refuses a taken id) or edits (`overwrite: true`) a project's name, description and
    /// instructions; its files are not touched.
    SaveProject {
        request_id: u64,
        project: ProjectDto,
        overwrite: bool,
    },
    /// Removes the project: only its `PROJECT.md`. The files stay in the vault as ordinary notes, and the conversations
    /// that were in it go on as conversations without a project.
    DeleteProject {
        request_id: u64,
        id: String,
    },
    /// The folders inside `path` on the hub's machine (P102), to pick a conversation's working folder. Folders only,
    /// never files. No `path` is where the person starts: the owner's home, a member's allowed folders. Answered by
    /// `DirList` or `DirError` with the same `request_id`.
    ListDirs {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
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
    /// Moves one of the person's conversations into a project (P103), or out of any with no `project_id`. The only way
    /// to change the project a conversation was started in. The project must exist in the person's vault, and a
    /// scheduled task's conversation can't be moved. Answered by `ConversationOk`/`ConversationError`; the next turn
    /// runs in the new scope, and what was already said stays in the conversation.
    MoveConversation {
        request_id: u64,
        conversation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
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
    /// The Warden API's keys (P12), answered by `ApiKeyList`. Open to any paired device, like the
    /// device list; creating and revoking ask for the pairing key again.
    ListApiKeys {
        request_id: u64,
    },
    /// Answered by `ApiKeyCreated`, the only time the key itself is sent.
    CreateApiKey {
        request_id: u64,
        pairing_key: String,
        name: String,
        /// Binds the key to this agent (it then only speaks as it); absent for a general key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// Answered by the updated `ApiKeyList`.
    RevokeApiKey {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// Nodes (P93), answered by `NodeList`. Open to any paired device; `SetNodeAccess` asks for the
    /// pairing key again.
    ListNodes {
        request_id: u64,
    },
    /// Writes this node's `[[nodes]]` entry: whether agents may use it, which ones, and whether every
    /// call waits for a yes.
    SetNodeAccess {
        request_id: u64,
        pairing_key: String,
        device_id: String,
        enabled: bool,
        #[serde(default)]
        agents: Vec<String>,
        #[serde(default)]
        require_approval: bool,
    },
    /// Scheduled tasks (P92), answered by `TaskList`. Open to any paired device; every change below
    /// asks for the pairing key again and is answered by the updated `TaskList`.
    ListTasks {
        request_id: u64,
    },
    /// The work agents delegated to each other in the background (P123), newest first, answered by `AgentTaskList`. Read
    /// only, for the owner: a member gets an empty list.
    ListAgentTasks {
        request_id: u64,
    },
    /// Pauses, resumes or stops (`action`: `pause`, `resume` or `cancel`) a task agents delegated (P123) that is running on this
    /// hub; the subtasks below it follow. Owner only and asks for the pairing key, like any change. Answered by the updated
    /// `AgentTaskList`, or by `TaskError` (a task that isn't running here, or a wrong key).
    ControlAgentTask {
        request_id: u64,
        pairing_key: String,
        task_id: String,
        action: String,
    },
    /// Changes the organization of the agents from the tree (P120): a position, a new report, a removal. Owner only, asks for the
    /// pairing key like a settings save, and is answered like one (`SettingsSaved` with the new settings, or `SettingsError`).
    EditAgentOrg {
        request_id: u64,
        pairing_key: String,
        edit: AgentOrgEdit,
    },
    /// Creates a task, or replaces `original_id` with it (a rename when the ids differ).
    SaveTask {
        request_id: u64,
        pairing_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        original_id: Option<String>,
        task: TaskDto,
    },
    /// Pauses or resumes a task.
    SetTaskEnabled {
        request_id: u64,
        pairing_key: String,
        id: String,
        enabled: bool,
    },
    DeleteTask {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// Runs a task now, on the hub, in the background; its conversation changing tells when it's done.
    RunTask {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// Incoming webhooks (P105), answered by `WebhookList`. Owner only. Open to any paired device; every change below asks
    /// for the pairing key again and is answered by the updated `WebhookList` (or `WebhookCreated`).
    ListWebhooks {
        request_id: u64,
    },
    /// Creates a webhook, or replaces `original_id` with it (a rename when the ids differ). A change of `auth` takes the
    /// credential away (it was of the other kind): make a new one.
    SaveWebhook {
        request_id: u64,
        pairing_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        original_id: Option<String>,
        webhook: WebhookDto,
    },
    /// Pauses or resumes a webhook.
    SetWebhookEnabled {
        request_id: u64,
        pairing_key: String,
        id: String,
        enabled: bool,
    },
    /// Removes a webhook and its credential. Its conversation stays.
    DeleteWebhook {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// Makes a new credential for a webhook — a token, or a signing secret when it wants `hmac` — replacing the old one.
    /// Answered by `WebhookCreated`, the only time the credential is shown.
    CreateWebhookCredential {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// Takes a webhook's credential away; its calls get 401 from the next one on.
    RevokeWebhookCredential {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// P84: the member on this connection picks their own password, answered by `PasswordChanged`
    /// or `UserError`. The only request a member on a provisional password may make.
    ChangePassword {
        request_id: u64,
        old_password: String,
        new_password: String,
        /// P84 fatia 4: needed only after the owner reset the password of a member whose data is
        /// encrypted — it's what opens the data, since the old password can't.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery_code: Option<String>,
    },
    /// P84 fatia 4: the member asks for a new recovery code (the old one stops working), with their
    /// password. Answered by `RecoveryCode` or `UserError`.
    RegenerateRecoveryCode {
        request_id: u64,
        password: String,
    },
    /// P84 fatia 4 parte B: the member says yes to the workspace's recovery policy when it changed to a
    /// weaker one, with their password (which opens their key). Answered by `RecoveryPolicyAccepted`
    /// or `UserError`.
    AcceptRecoveryPolicy {
        request_id: u64,
        password: String,
    },
    /// The member has seen the recoveries the owner made. Answered by `RecoveryNoticesAcked`.
    AckRecoveryNotices {
        request_id: u64,
    },
    /// A member chooses whether the assistant may learn from their conversations (P115): `enabled:
    /// false` is the opt-out. The workspace's `[learning]` still has to be on. Answered by
    /// `LearningSet` or `UserError`.
    SetLearning {
        request_id: u64,
        enabled: bool,
    },
    /// The owner sets the workspace's recovery policy (`private`, `consent` or `company`), repeating
    /// the pairing key. `new_key` replaces the owner's recovery key. Answered by `RecoveryPolicy` or
    /// `UserError`.
    SetRecoveryPolicy {
        request_id: u64,
        pairing_key: String,
        policy: String,
        #[serde(default)]
        new_key: bool,
    },
    /// The owner opens a member's data with the workspace's recovery key (`consent` also needs the
    /// person's `code`) and gives them a new provisional password. Answered by `UserList` with the
    /// password once, or `UserError`.
    RecoverMember {
        request_id: u64,
        pairing_key: String,
        id: String,
        recovery_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// The workspace's members, answered by `UserList`. The root's only.
    ListUsers {
        request_id: u64,
    },
    /// Creates a member (`is_new`) with a provisional password, or renames one. Answered by
    /// `UserList`, with the provisional password once for a new member.
    SaveUser {
        request_id: u64,
        pairing_key: String,
        id: String,
        name: String,
        #[serde(default)]
        is_new: bool,
    },
    /// Gives a member a new provisional password (a forgotten one), shown once in `UserList`.
    ResetPassword {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// P84 fatia 5: the owner makes an invite for a member to link their TruthID, shown once in
    /// `UserList` (`invite_code`). A new one replaces an open one.
    CreateInvite {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// The owner unties a member's TruthID, or cancels an open invite. Answered by `UserList`.
    UnlinkTruthId {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// A member links their TruthID with the owner's invite `code`, naming their TruthID
    /// `username`. Answered by `TruthIdLinked` or `UserError`.
    RedeemInvite {
        request_id: u64,
        code: String,
        username: String,
    },
    /// P84 fatia 4: the owner brings back a member they removed whose encrypted data was kept, with the
    /// password they had. Their devices were revoked when they were removed: they pair again. Answered
    /// by `UserList`.
    RestoreUser {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// Removes a member and revokes their devices. Their vault and conversations stay on the hub's disk.
    RemoveUser {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// P84 fatia 2: the tools a member may use (`None`: the safe default). Answered by `UserList`.
    SetUserTools {
        request_id: u64,
        pairing_key: String,
        id: String,
        #[serde(default)]
        tools: Option<Vec<String>>,
    },
    /// P102: the folders a member may pick as a working folder, on the hub's machine and on nodes; empty lists take them
    /// all away. Answered by `UserList`, or `UserError` for a path that isn't valid.
    SetUserWorkdirs {
        request_id: u64,
        pairing_key: String,
        id: String,
        #[serde(default)]
        workdirs: Vec<String>,
        #[serde(default)]
        node_workdirs: Vec<NodeFolderDto>,
    },
    /// P115: the model the assistant's learning uses for a member, a provider or combo id (`None`: the
    /// workspace's `[learning] provider`). Answered by `UserList`.
    SetUserLearningProvider {
        request_id: u64,
        pairing_key: String,
        id: String,
        #[serde(default)]
        provider: Option<String>,
    },
    /// A member creates (`original_id` absent) or edits one of their own agents. Answered by
    /// `Settings` (the member's view) or `SettingsError`. Whatever it asks, the agent stays theirs,
    /// unshared, without the `can_*` powers, and within their tools.
    SaveOwnAgent {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        original_id: Option<String>,
        agent: AgentSettingsDto,
    },
    /// A member deletes one of their own agents. Answered like `SaveOwnAgent`.
    DeleteOwnAgent {
        request_id: u64,
        id: String,
    },
    /// P84 fatia 3: the shared spaces, answered by `SpaceList` — all of them for the owner, the ones
    /// they're in for a member.
    ListSpaces {
        request_id: u64,
    },
    /// The owner creates (`original_id` absent) or edits a space. Answered by `SpaceList` or `UserError`.
    SaveSpace {
        request_id: u64,
        pairing_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        original_id: Option<String>,
        space: SpaceDto,
    },
    /// The owner stops sharing a folder. The folder and its notes stay in their vault.
    DeleteSpace {
        request_id: u64,
        pairing_key: String,
        id: String,
    },
    /// P117: the strangers waiting to talk to the Telegram or WhatsApp bot, answered by `BotPairings`.
    /// The root's connection only.
    ListBotPairings {
        request_id: u64,
    },
    /// The owner approves (`approve`: the sender joins the bot's allow-list) or denies the request
    /// behind `code`. Answered by `BotPairings` with what is still waiting, or `UserError`.
    /// `member`: approve the chat as speaking as that member of the workspace (it must be linked to the
    /// bots); absent, it is answered as the owner, as before. Denying ignores it.
    ResolveBotPairing {
        request_id: u64,
        pairing_key: String,
        code: String,
        approve: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        member: Option<String>,
    },
    /// Checks a model provider's key without spending a conversation (P10), answered by `ProviderTest` or
    /// `UserError`. `provider` is the form as the screen has it: `Keep` on its key uses the saved one (found by
    /// `original_id`), `Set` is a key typed but not saved (so only over an encrypted or local connection). The hub
    /// only asks an address it already has saved, so a paired device can't make it call somewhere new. Root only.
    TestProvider {
        request_id: u64,
        pairing_key: String,
        provider: ProviderEditDto,
    },
    /// The hub's sync state (P61), answered by `SyncStatus`. Open to any paired device, like
    /// reading settings.
    RequestSyncStatus {
        request_id: u64,
    },
    /// Runs a sync round, or sets up the hub's vault key, answered by the updated `SyncStatus` or
    /// a `SyncError`. `pairing_key` is asked every time, as in `SaveSettings`.
    SyncAction {
        request_id: u64,
        pairing_key: String,
        action: SyncActionDto,
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
        ClientMessage::Chat { message: message.into(), conversation_id: None, attachments: Vec::new(), agent_id: None, project_id: None, workdir: None }
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
        /// P84: the member this device belongs to — absent for the root (the pairing key's
        /// devices), which is also what a hub from before P84 sends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<UserInfoDto>,
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
        /// The turn's provider failed and a reserve answered (P79) — for the discreet line above
        /// the answer. Absent almost always; clients that don't know it ignore it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fallbacks: Vec<ProviderFallbackDto>,
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
    /// What a code engine is doing in the middle of a code project's turn (P103 b): sent any number of times between the
    /// `Chat` and its `ChatResponse`, for the client to show. Clients that don't know it ignore it.
    ChatEvent {
        conversation_id: String,
        event: ChatEventDto,
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
    /// Asks a node to run one model call on its provider `model` (P93) — answered by
    /// `ModelEvent`s and a `ModelDone`/`ModelError` with the same `request_id`.
    ModelRequest {
        request_id: u64,
        model: String,
        messages: Vec<Message>,
        #[serde(default)]
        tools: Vec<ToolSpec>,
    },
    /// The hub no longer wants that answer (the turn ended or dropped it).
    ModelCancel {
        request_id: u64,
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
    /// Reply to `ClientMessage::ListProjects`.
    ProjectList {
        request_id: u64,
        projects: Vec<ProjectDto>,
    },
    /// Reply to `ClientMessage::ListDirs`: the folders in `path`, and the one above it (`None` at the top of what the
    /// person may see). `path` is empty for a member's list of allowed folders.
    DirList {
        request_id: u64,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        dirs: Vec<DirEntryDto>,
    },
    /// A `ListDirs` failed (not a folder, outside what the person may see, unreadable).
    DirError {
        request_id: u64,
        message: String,
    },
    /// Reply to a successful `SaveProject`/`DeleteProject`.
    ProjectOk {
        request_id: u64,
    },
    /// A `ListProjects`/`SaveProject`/`DeleteProject` failed (invalid project, id taken, no such project).
    ProjectError {
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
    /// Reply to a successful `RenameConversation`/`DeleteConversation`/`MoveConversation`.
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
    /// Reply to `ListApiKeys` and to a successful `RevokeApiKey`, oldest first.
    ApiKeyList {
        request_id: u64,
        keys: Vec<ApiKeyDto>,
    },
    /// Reply to `CreateApiKey`: `key` is shown once and kept nowhere on the hub.
    ApiKeyCreated {
        request_id: u64,
        key: String,
        keys: Vec<ApiKeyDto>,
    },
    /// An API key request failed. `auth_rejected`: the pairing key was wrong; nothing changed.
    ApiKeyError {
        request_id: u64,
        message: String,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// Reply to `ListNodes` and to a successful `SetNodeAccess` (P93).
    NodeList {
        request_id: u64,
        nodes: Vec<NodeInfoDto>,
    },
    /// A node request failed. `auth_rejected`: the pairing key was wrong; nothing changed.
    NodeError {
        request_id: u64,
        message: String,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// Reply to every task request (P92), in the config's order. `runs_here`: this hub runs the
    /// tasks on schedule (`--run-tasks`, or the desktop's switch).
    TaskList {
        request_id: u64,
        tasks: Vec<TaskInfoDto>,
        runs_here: bool,
    },
    /// Reply to `ListAgentTasks` (P123): the delegated tasks, newest first.
    AgentTaskList {
        request_id: u64,
        tasks: Vec<AgentTaskDto>,
    },
    /// A task request failed. `auth_rejected`: the pairing key was wrong; nothing changed.
    TaskError {
        request_id: u64,
        message: String,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// Reply to every webhook request but a new credential (P105), in the config's order. `serves_here`: this hub takes
    /// the calls (`/hooks/<id>`); without it the list is only a config.
    WebhookList {
        request_id: u64,
        webhooks: Vec<WebhookInfoDto>,
        serves_here: bool,
    },
    /// Reply to `CreateWebhookCredential`: `credential` is the token or signing secret, shown once and never sent again
    /// (a token isn't even kept in the clear on the hub). `kind` is `"token"` or `"hmac"`.
    WebhookCreated {
        request_id: u64,
        id: String,
        credential: String,
        kind: String,
        webhooks: Vec<WebhookInfoDto>,
        serves_here: bool,
    },
    /// A webhook request failed. `auth_rejected`: the pairing key was wrong; nothing changed.
    WebhookError {
        request_id: u64,
        message: String,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// P84: the members, after `ListUsers` or a change. `temp_password`: the provisional password
    /// of the member just created or reset — shown once, never stored in the clear.
    UserList {
        request_id: u64,
        users: Vec<UserInfoDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        temp_password: Option<String>,
        /// P84 fatia 5: the invite just made, shown once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invite_code: Option<String>,
        /// P84 fatia 4 parte B: the workspace's recovery policy (empty from a hub that predates it).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        recovery_policy: String,
        /// Members the owner removed whose encrypted data is still on disk: `RestoreUser` brings them back.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        removed: Vec<RemovedUserDto>,
    },
    /// The workspace's recovery policy after `SetRecoveryPolicy`. `secret` is the owner's recovery key,
    /// present only when one was just made — shown once and never kept: it's typed in for each recovery.
    RecoveryPolicy {
        request_id: u64,
        policy: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
    },
    /// The member accepted the policy: their data follows it now. `recovery_code`: entering or leaving
    /// `consent` made a new one — shown once.
    RecoveryPolicyAccepted {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery_code: Option<String>,
    },
    RecoveryNoticesAcked {
        request_id: u64,
    },
    LearningSet {
        request_id: u64,
    },
    /// The answer to a `Hello` with `truthid_login`: what the TruthID app scans (`payload` is the QR's JSON),
    /// good until `expires_at_ms` (the app refuses an older challenge). The `HelloAck` follows once it approves.
    TruthIdChallenge {
        payload: String,
        expires_at_ms: u64,
    },
    /// The member's TruthID is linked (`RedeemInvite`).
    TruthIdLinked {
        request_id: u64,
        username: String,
    },
    PasswordChanged {
        request_id: u64,
        /// P84 fatia 4: the member's data is encrypted from this change on, and this is the
        /// recovery code that opens it if they lose the password. Sent once, never stored in the
        /// clear: the client has to make them write it down.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery_code: Option<String>,
    },
    /// A recovery code, shown once: the answer to `RegenerateRecoveryCode`, or (`request_id` 0)
    /// sent right after `HelloAck` when signing in turned encryption on for a member from before.
    RecoveryCode {
        request_id: u64,
        code: String,
    },
    /// P84 fatia 3: the shared spaces.
    SpaceList {
        request_id: u64,
        spaces: Vec<SpaceDto>,
    },
    /// P117: the strangers waiting for the owner to let them talk to a bot, oldest first.
    BotPairings {
        request_id: u64,
        pairings: Vec<BotPairingDto>,
        /// Who a chat may be approved as speaking as, and whether the bots are linked to them.
        #[serde(default)]
        members: Vec<BotMemberDto>,
    },
    /// What testing a provider's key came to (P10). `ok` only when the provider accepted it; `kind` is one of `ok`,
    /// `unverifiable`, `rejected`, `rate_limited`, `provider_down`, `unreachable`, `unsupported`; `message` is one
    /// sentence that never carries the key or what the provider answered.
    ProviderTest {
        request_id: u64,
        ok: bool,
        kind: String,
        message: String,
    },
    /// A user request failed. `auth_rejected`: the pairing key was wrong, or this connection isn't
    /// the root's; nothing changed.
    UserError {
        request_id: u64,
        message: String,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// A device request failed. `auth_rejected`: the pairing key was wrong; nothing changed.
    DeviceError {
        request_id: u64,
        message: String,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// Reply to `RequestSyncStatus` and to a successful `SyncAction`. `last_round` is how the
    /// round a "sync now" ran went — an error there is the round's, not the request's.
    SyncStatus {
        request_id: u64,
        status: SyncStatusDto,
        /// Only in the reply to a `PairHost`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pairing_code: Option<String>,
    },
    /// A sync request failed. `auth_rejected`: the pairing key was wrong; nothing ran.
    SyncError {
        request_id: u64,
        message: String,
        #[serde(default)]
        auth_rejected: bool,
    },
    /// A tool in this client's `Chat` turn needs the person's yes (P46 `manage_agents`, SSH hosts
    /// with `require_approval`) — `warden_core::tool::ApprovalRequest` on the wire. Answered by
    /// `ResolveApproval`; no answer before the hub's deadline counts as no.
    ApprovalRequest {
        approval_id: u64,
        target: String,
        action: String,
        detail: String,
        /// What an "always" answer would cover (P103 b: `git status *`), when this ask can be answered that way. The
        /// client then offers a button for it and answers with `ResolveApproval.always`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        always: Option<String>,
        /// P122: the kind of action this agent has to get approved (`critical_infra`, `delete_data`...), when the ask
        /// comes from that rule and not from the tool's own. A client that doesn't know the field just doesn't show it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<String>,
    },
    /// The hub stopped waiting for `approval_id` (deadline reached): the client should close it.
    ApprovalCancelled {
        approval_id: u64,
    },
    /// One of this device's conversations was created or changed outside a `Chat` reply — an agent
    /// left a message for another (P46 `message_agent`), or answered one. The client reloads its list
    /// (and the conversation, if it's open).
    ConversationsChanged {
        conversation_id: String,
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
            node: None,
            username: None,
            password: None,
            recovery_codes: false,
            truthid_login: false,
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
        let without = ServerMessage::HelloAck { server_name: "Hub".into(), device_token: None, user: None };
        assert_eq!(serde_json::to_string(&without).unwrap(), r#"{"type":"helloAck","serverName":"Hub"}"#);

        let with = ServerMessage::HelloAck { server_name: "Hub".into(), device_token: Some("tok".into()), user: None };
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
            node: None,
            username: None,
            password: None,
            recovery_codes: false,
            truthid_login: false,
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
            fallbacks: Vec::new(),
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
            skill: SkillDto { name: "review-pr".into(), description: "d".into(), body: "b".into(), agents: vec!["writer".into()], proposed: false, source: None, proposed_at: None, revises: None },
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
            skills: vec![SkillDto { name: "x".into(), description: "d".into(), body: "b".into(), agents: Vec::new(), proposed: false, source: None, proposed_at: None, revises: None }],
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

        let with = ClientMessage::Chat { message: "hi".into(), conversation_id: Some("c1".into()), attachments: Vec::new(), agent_id: None, project_id: None, workdir: None };
        let json = serde_json::to_string(&with).unwrap();
        assert_eq!(json, r#"{"type":"chat","message":"hi","conversationId":"c1"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), with);
    }

    /// P103: a chat names the project it starts in; the project messages and the summary's `projectId` use the names
    /// the web and the desktop expect, and everything from before projects (no field) still reads.
    #[test]
    fn projects_travel_with_the_names_the_clients_expect_and_old_peers_still_read() {
        let chat = ClientMessage::Chat { message: "hi".into(), conversation_id: Some("c1".into()), attachments: Vec::new(), agent_id: None, project_id: Some("tax".into()), workdir: None };
        let json = serde_json::to_string(&chat).unwrap();
        assert_eq!(json, r#"{"type":"chat","message":"hi","conversationId":"c1","projectId":"tax"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), chat);

        let save = ClientMessage::SaveProject { request_id: 1, project: ProjectDto { id: "tax".into(), name: "Tax".into(), description: "d".into(), instructions: "i".into(), workdir: None, code: false }, overwrite: false };
        let json = serde_json::to_string(&save).unwrap();
        assert_eq!(json, r#"{"type":"saveProject","requestId":1,"project":{"id":"tax","name":"Tax","description":"d","instructions":"i"},"overwrite":false}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), save);
        assert_eq!(serde_json::to_string(&ClientMessage::ListProjects { request_id: 2 }).unwrap(), r#"{"type":"listProjects","requestId":2}"#);
        assert_eq!(serde_json::to_string(&ClientMessage::DeleteProject { request_id: 3, id: "tax".into() }).unwrap(), r#"{"type":"deleteProject","requestId":3,"id":"tax"}"#);

        let list = ServerMessage::ProjectList { request_id: 2, projects: vec![ProjectDto { id: "tax".into(), name: "Tax".into(), description: String::new(), instructions: String::new(), workdir: None, code: false }] };
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(json, r#"{"type":"projectList","requestId":2,"projects":[{"id":"tax","name":"Tax","description":"","instructions":""}]}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), list);
        assert_eq!(serde_json::to_string(&ServerMessage::ProjectOk { request_id: 4 }).unwrap(), r#"{"type":"projectOk","requestId":4}"#);
        assert_eq!(serde_json::to_string(&ServerMessage::ProjectError { request_id: 5, message: "m".into() }).unwrap(), r#"{"type":"projectError","requestId":5,"message":"m"}"#);

        // A client from before projects sends no `projectId`; a hub from before them sends none in a summary.
        let old_chat = serde_json::from_str::<ClientMessage>(r#"{"type":"chat","message":"hi"}"#).unwrap();
        assert!(matches!(old_chat, ClientMessage::Chat { project_id: None, .. }));
        let summary = serde_json::from_str::<ConversationSummary>(r#"{"id":"c","title":"t","createdAt":1,"updatedAt":2}"#).unwrap();
        assert_eq!(summary.project_id, None);
        let grouped = ConversationSummary { project_id: Some("tax".into()), ..summary };
        assert!(serde_json::to_string(&grouped).unwrap().contains(r#""projectId":"tax""#));
    }

    /// P102: a chat names the folder it starts in, the folder browser has its own messages, and everything from before
    /// folders (no field) still reads.
    #[test]
    fn working_folders_travel_with_the_names_the_clients_expect_and_old_peers_still_read() {
        let chat = ClientMessage::Chat { message: "hi".into(), conversation_id: Some("c1".into()), attachments: Vec::new(), agent_id: None, project_id: None, workdir: Some("/srv/work".into()) };
        let json = serde_json::to_string(&chat).unwrap();
        assert_eq!(json, r#"{"type":"chat","message":"hi","conversationId":"c1","workdir":"/srv/work"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), chat);
        let old_chat = serde_json::from_str::<ClientMessage>(r#"{"type":"chat","message":"hi"}"#).unwrap();
        assert!(matches!(old_chat, ClientMessage::Chat { workdir: None, .. }));

        assert_eq!(serde_json::to_string(&ClientMessage::ListDirs { request_id: 1, path: None }).unwrap(), r#"{"type":"listDirs","requestId":1}"#);
        let open = ClientMessage::ListDirs { request_id: 2, path: Some("/srv".into()) };
        let json = serde_json::to_string(&open).unwrap();
        assert_eq!(json, r#"{"type":"listDirs","requestId":2,"path":"/srv"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), open);

        let list = ServerMessage::DirList { request_id: 2, path: "/srv".into(), parent: Some("/".into()), dirs: vec![DirEntryDto { name: "work".into(), path: "/srv/work".into() }] };
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(json, r#"{"type":"dirList","requestId":2,"path":"/srv","parent":"/","dirs":[{"name":"work","path":"/srv/work"}]}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), list);
        let top = ServerMessage::DirList { request_id: 3, path: String::new(), parent: None, dirs: Vec::new() };
        assert_eq!(serde_json::to_string(&top).unwrap(), r#"{"type":"dirList","requestId":3,"path":"","dirs":[]}"#);
        assert_eq!(serde_json::to_string(&ServerMessage::DirError { request_id: 4, message: "m".into() }).unwrap(), r#"{"type":"dirError","requestId":4,"message":"m"}"#);

        let summary = serde_json::from_str::<ConversationSummary>(r#"{"id":"c","title":"t","createdAt":1,"updatedAt":2}"#).unwrap();
        assert_eq!(summary.workdir, None);
        let in_folder = ConversationSummary { workdir: Some("/srv/work".into()), ..summary };
        assert!(serde_json::to_string(&in_folder).unwrap().contains(r#""workdir":"/srv/work""#));
    }

    #[test]
    fn the_delegated_tasks_are_asked_for_and_listed_with_what_they_cost() {
        let ask = ClientMessage::ListAgentTasks { request_id: 4 };
        let json = serde_json::to_string(&ask).unwrap();
        assert_eq!(json, r#"{"type":"listAgentTasks","requestId":4}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), ask);

        let task = AgentTaskDto {
            id: "at-1".into(),
            group: "turn-1".into(),
            owner: Some("chief".into()),
            assignee: "backend".into(),
            parent_id: Some("at-0".into()),
            objective: "build the API".into(),
            model: Some("strong".into()),
            channel: "desktop".into(),
            state: "done".into(),
            result: Some("done".into()),
            error: None,
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            total_tokens: Some(15),
            created_at_ms: 1,
            started_at_ms: Some(2),
            finished_at_ms: Some(3),
            controllable: false,
        };
        let reply = ServerMessage::AgentTaskList { request_id: 4, tasks: vec![task.clone()] };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            json,
            r#"{"type":"agentTaskList","requestId":4,"tasks":[{"id":"at-1","group":"turn-1","owner":"chief","assignee":"backend","parentId":"at-0","objective":"build the API","model":"strong","channel":"desktop","state":"done","result":"done","promptTokens":10,"completionTokens":5,"totalTokens":15,"createdAtMs":1,"startedAtMs":2,"finishedAtMs":3}]}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), reply);

        // A pending task has none of the fields that come later.
        let pending: AgentTaskDto = serde_json::from_str(r#"{"id":"a","group":"g","assignee":"x","objective":"o","channel":"cli","state":"pending","createdAtMs":9}"#).unwrap();
        assert_eq!((pending.owner, pending.total_tokens, pending.started_at_ms), (None, None, None));

        // A running task of this process says it can be controlled, and the control message is asked with the pairing key.
        let running = serde_json::to_string(&AgentTaskDto { state: "running".into(), controllable: true, ..task }).unwrap();
        assert!(running.contains(r#""controllable":true"#), "{running}");
        let control = ClientMessage::ControlAgentTask { request_id: 5, pairing_key: "k".into(), task_id: "at-1".into(), action: "pause".into() };
        let json = serde_json::to_string(&control).unwrap();
        assert_eq!(json, r#"{"type":"controlAgentTask","requestId":5,"pairingKey":"k","taskId":"at-1","action":"pause"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), control);
    }

    #[test]
    fn a_change_to_the_organization_is_asked_with_the_pairing_key() {
        let ask = ClientMessage::EditAgentOrg {
            request_id: 6,
            pairing_key: "k".into(),
            edit: AgentOrgEdit::SetPosition { id: "dev".into(), role: Some("Backend".into()), reports_to: Some("lead".into()) },
        };
        let json = serde_json::to_string(&ask).unwrap();
        assert_eq!(json, r#"{"type":"editAgentOrg","requestId":6,"pairingKey":"k","edit":{"kind":"setPosition","id":"dev","role":"Backend","reportsTo":"lead"}}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), ask);
        let top = r#"{"type":"editAgentOrg","requestId":7,"pairingKey":"k","edit":{"kind":"remove","id":"dev"}}"#;
        assert!(matches!(serde_json::from_str::<ClientMessage>(top).unwrap(), ClientMessage::EditAgentOrg { edit: AgentOrgEdit::Remove { .. }, .. }));
    }

    #[test]
    fn a_chat_can_name_an_agent_and_approvals_round_trip() {
        let chat = ClientMessage::Chat { message: "hi".into(), conversation_id: None, attachments: Vec::new(), agent_id: Some("chief".into()), project_id: None, workdir: None };
        let json = serde_json::to_string(&chat).unwrap();
        assert_eq!(json, r#"{"type":"chat","message":"hi","agentId":"chief"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), chat);

        let ask = ServerMessage::ApprovalRequest { approval_id: 3, target: "poet".into(), action: "create_agent".into(), detail: "d".into(), always: None, category: None };
        let json = serde_json::to_string(&ask).unwrap();
        assert_eq!(json, r#"{"type":"approvalRequest","approvalId":3,"target":"poet","action":"create_agent","detail":"d"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), ask);

        let answer = serde_json::from_str::<ClientMessage>(r#"{"type":"resolveApproval","approvalId":3,"approved":true}"#).unwrap();
        assert_eq!(answer, ClientMessage::ResolveApproval { approval_id: 3, approved: true, always: false });

        // P103 b: an ask that can be "always" says what that covers, and the answer can take it.
        let offered = ServerMessage::ApprovalRequest { approval_id: 4, target: "repo".into(), action: "bash".into(), detail: "git status -s".into(), always: Some("git status *".into()), category: None };
        let json = serde_json::to_string(&offered).unwrap();
        assert_eq!(json, r#"{"type":"approvalRequest","approvalId":4,"target":"repo","action":"bash","detail":"git status -s","always":"git status *"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), offered);

        // P122: an ask that comes from an agent's own rule says which kind of action it is.
        let by_rule = ServerMessage::ApprovalRequest { approval_id: 5, target: "shell".into(), action: "tool_call".into(), detail: "{}".into(), always: None, category: Some("critical_infra".into()) };
        let json = serde_json::to_string(&by_rule).unwrap();
        assert_eq!(json, r#"{"type":"approvalRequest","approvalId":5,"target":"shell","action":"tool_call","detail":"{}","category":"critical_infra"}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), by_rule);
        let always = serde_json::from_str::<ClientMessage>(r#"{"type":"resolveApproval","approvalId":4,"approved":true,"always":true}"#).unwrap();
        assert_eq!(always, ClientMessage::ResolveApproval { approval_id: 4, approved: true, always: true });
        let cancelled = serde_json::to_string(&ServerMessage::ApprovalCancelled { approval_id: 3 }).unwrap();
        assert_eq!(cancelled, r#"{"type":"approvalCancelled","approvalId":3}"#);
        let changed = serde_json::to_string(&ServerMessage::ConversationsChanged { conversation_id: "c".into() }).unwrap();
        assert_eq!(changed, r#"{"type":"conversationsChanged","conversationId":"c"}"#);

        // P103 b: a code engine's work as it happens, and the stop.
        let tool = ServerMessage::ChatEvent { conversation_id: "c".into(), event: ChatEventDto::Tool { call_id: "k1".into(), tool: "bash".into(), title: "cargo test".into(), status: ToolStatusDto::Running } };
        let json = serde_json::to_string(&tool).unwrap();
        assert_eq!(json, r#"{"type":"chatEvent","conversationId":"c","event":{"type":"tool","callId":"k1","tool":"bash","title":"cargo test","status":"running"}}"#);
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), tool);
        let text = serde_json::to_string(&ServerMessage::ChatEvent { conversation_id: "c".into(), event: ChatEventDto::from_code(CodeEvent::Text("hi".into())).unwrap() }).unwrap();
        assert_eq!(ChatEventDto::from_code(CodeEvent::Session("s".into())), None, "the session is not for showing");
        assert_eq!(text, r#"{"type":"chatEvent","conversationId":"c","event":{"type":"text","text":"hi"}}"#);
        let cancel = ClientMessage::CancelTurn { conversation_id: "c".into() };
        assert_eq!(serde_json::to_string(&cancel).unwrap(), r#"{"type":"cancelTurn","conversationId":"c"}"#);
        let mode = ClientMessage::SetCodeMode { conversation_id: "c".into(), mode: "plan".into() };
        assert_eq!(serde_json::to_string(&mode).unwrap(), r#"{"type":"setCodeMode","conversationId":"c","mode":"plan"}"#);
        assert_eq!(serde_json::from_str::<ClientMessage>(r#"{"type":"setCodeMode","conversationId":"c","mode":"plan"}"#).unwrap(), mode);
    }

    #[test]
    fn a_chat_with_attachments_and_transcription_messages_round_trip() {
        let chat = ClientMessage::Chat {
            message: String::new(),
            conversation_id: None,
            attachments: vec![Attachment { mime_type: "application/pdf".into(), data: "JVBE".into() }],
            agent_id: None,
            project_id: None,
            workdir: None,
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
            (
                ClientMessage::MoveConversation { request_id: 4, conversation_id: "c1".into(), project_id: Some("tax".into()) },
                r#"{"type":"moveConversation","requestId":4,"conversationId":"c1","projectId":"tax"}"#,
            ),
            // No `projectId` is "out of any project".
            (
                ClientMessage::MoveConversation { request_id: 5, conversation_id: "c1".into(), project_id: None },
                r#"{"type":"moveConversation","requestId":5,"conversationId":"c1"}"#,
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
                    conversations: vec![ConversationSummary { id: "c1".into(), title: "Trip".into(), created_at: 1, updated_at: 2, agent_id: None, project_id: None, workdir: None }],
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
                daily_cost: vec![DailyCostDto { date: "2026-09-25".into(), calls: 1, cost_usd: 0.5, unpriced_calls: 0 }],
                limits_enabled: true,
                limits: vec![limit.clone()],
                recent: Some(RecentSpendDto {
                    window_hours: 24,
                    by_model: vec![SpendBucketDto { key: "m".into(), calls: 1, tokens: 10, cost_usd: 0.5, unpriced_calls: 0 }],
                    by_channel: Vec::new(),
                    by_provider: vec![SpendBucketDto { key: "main".into(), calls: 1, tokens: 10, cost_usd: 0.5, unpriced_calls: 0 }],
                    by_agent: vec![SpendBucketDto { key: "writer".into(), calls: 1, tokens: 10, cost_usd: 0.5, unpriced_calls: 0 }],
                    by_person: Vec::new(),
                }),
                ledger_error: None,
            },
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r#""dailyCost":[{"date":"2026-09-25","calls":1,"costUsd":0.5,"unpricedCalls":0}]"#), "{json}");
        assert!(json.contains(r#""byProvider":[{"key":"main""#) && json.contains(r#""byAgent":[{"key":"writer""#), "{json}");
        // A hub from before these existed sends none of them: they read as empty rather than failing.
        let old = r#"{"windowHours":24,"byModel":[],"byChannel":[]}"#;
        let recent: RecentSpendDto = serde_json::from_str(old).unwrap();
        assert!(recent.by_provider.is_empty() && recent.by_agent.is_empty() && recent.by_person.is_empty());
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

    /// P117: the shapes the web client sends and reads for the bots' pairing requests.
    #[test]
    fn bot_pairing_messages_use_the_names_the_web_expects() {
        let resolve = ClientMessage::ResolveBotPairing { request_id: 4, pairing_key: "k".into(), code: "ABCD-EFGH".into(), approve: true, member: None };
        let json = serde_json::to_value(&resolve).unwrap();
        assert_eq!(json, serde_json::json!({ "type": "resolveBotPairing", "requestId": 4, "pairingKey": "k", "code": "ABCD-EFGH", "approve": true }), "no member, no field");
        assert_eq!(serde_json::from_value::<ClientMessage>(json).unwrap(), resolve);
        // Approving as a member of the workspace (P117): the member travels by name.
        let as_ana = ClientMessage::ResolveBotPairing { request_id: 4, pairing_key: "k".into(), code: "ABCD-EFGH".into(), approve: true, member: Some("ana".into()) };
        let json = serde_json::to_value(&as_ana).unwrap();
        assert_eq!(json["member"], "ana");
        assert_eq!(serde_json::from_value::<ClientMessage>(json).unwrap(), as_ana);
        let list: ClientMessage = serde_json::from_str(r#"{"type":"listBotPairings","requestId":5}"#).unwrap();
        assert_eq!(list, ClientMessage::ListBotPairings { request_id: 5 });

        let waiting = ServerMessage::BotPairings {
            request_id: 5,
            pairings: vec![BotPairingDto { channel: "telegram".into(), sender: "42".into(), label: "ana".into(), code: "ABCD-EFGH".into(), expires_at: 1_700_000_000 }],
            members: vec![BotMemberDto { id: "ana".into(), name: "Ana".into(), linked: true }, BotMemberDto { id: "bia".into(), name: "Bia".into(), linked: false }],
        };
        let json = serde_json::to_value(&waiting).unwrap();
        assert_eq!(json["type"], "botPairings");
        assert_eq!(json["pairings"][0]["expiresAt"], 1_700_000_000u64);
        assert_eq!(json["members"], serde_json::json!([{ "id": "ana", "name": "Ana", "linked": true }, { "id": "bia", "name": "Bia", "linked": false }]));
        assert_eq!(serde_json::from_value::<ServerMessage>(json).unwrap(), waiting);

        // A hub from before it sends no `members`, and a client from before it sends no `member`: both still read.
        let old: ServerMessage = serde_json::from_str(r#"{"type":"botPairings","requestId":5,"pairings":[]}"#).unwrap();
        assert_eq!(old, ServerMessage::BotPairings { request_id: 5, pairings: Vec::new(), members: Vec::new() });
        let old: ClientMessage = serde_json::from_str(r#"{"type":"resolveBotPairing","requestId":4,"pairingKey":"k","code":"ABCD-EFGH","approve":false}"#).unwrap();
        assert_eq!(old, ClientMessage::ResolveBotPairing { request_id: 4, pairing_key: "k".into(), code: "ABCD-EFGH".into(), approve: false, member: None });
    }

    /// P10: the shapes the web sends and reads for "Test key".
    #[test]
    fn provider_test_messages_use_the_names_the_web_expects() {
        let ask = ClientMessage::TestProvider {
            request_id: 6,
            pairing_key: "k".into(),
            provider: ProviderEditDto { original_id: Some("main".into()), id: "main".into(), kind: "gemini".into(), base_url: String::new(), model: String::new(), api_key: SecretEdit::Keep, node: String::new() },
        };
        let json = serde_json::to_value(&ask).unwrap();
        assert_eq!(json["type"], "testProvider");
        assert_eq!((json["requestId"].clone(), json["provider"]["originalId"].clone(), json["provider"]["apiKey"].clone()), (6.into(), "main".into(), serde_json::json!({ "action": "keep" })));
        assert_eq!(serde_json::from_value::<ClientMessage>(json).unwrap(), ask);

        let answer = ServerMessage::ProviderTest { request_id: 6, ok: false, kind: "rejected".into(), message: "Gemini rejected the key.".into() };
        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(json, serde_json::json!({ "type": "providerTest", "requestId": 6, "ok": false, "kind": "rejected", "message": "Gemini rejected the key." }));
        assert_eq!(serde_json::from_value::<ServerMessage>(json).unwrap(), answer);
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
                node: String::new(),
            }],
            active_provider: "primary".into(),
            agents: Vec::new(),
            tavily_key: SecretEdit::Keep,
            whisper_key: SecretEdit::Clear,
            limits: None,
            prices: Vec::new(),
            git_sync: None,
            combos: None,
            model_policies: None,
            bots: Some(BotsSettingsDto { telegram_allowed_users: vec![42], ..BotsSettingsDto::default() }),
            telegram_token: SecretEdit::Keep,
            advanced: None,
            machine: None,
        };
        assert!(update.sets_a_secret());
        let token_only = HubSettingsUpdate {
            providers: Vec::new(),
            tavily_key: SecretEdit::Keep,
            whisper_key: SecretEdit::Keep,
            git_sync: Some(GitSyncEditDto { remote_url: "https://g/v.git".into(), token: SecretEdit::Set("t".into()) }),
            ..update.clone()
        };
        assert!(token_only.sets_a_secret(), "a git token is a secret too");
        let nothing = HubSettingsUpdate { providers: Vec::new(), whisper_key: SecretEdit::Keep, git_sync: None, ..update.clone() };
        assert!(!nothing.sets_a_secret());
        assert!(HubSettingsUpdate { telegram_token: SecretEdit::Set("123:abc".into()), ..nothing.clone() }.sets_a_secret(), "the Telegram token is a secret");
        assert!(!HubSettingsUpdate { telegram_token: SecretEdit::Clear, ..nothing.clone() }.sets_a_secret(), "clearing it sends nothing new");
        let machine = |env: SecretEdit| MachineEditDto {
            enable_shell: false,
            vault_path: String::new(),
            generated_path: String::new(),
            mcp_servers: vec![McpServerEditDto {
                original_name: None,
                name: "notes".into(),
                kind: "stdio".into(),
                command: "npx".into(),
                args: Vec::new(),
                env: vec![SecretEntryEdit { key: "TOKEN".into(), value: env }],
                url: String::new(),
                headers: Vec::new(),
            }],
            ssh_hosts: Vec::new(),
            embedded_server: None,
        };
        assert!(HubSettingsUpdate { machine: Some(Box::new(machine(SecretEdit::Set("s".into())))), ..nothing.clone() }.sets_a_secret(), "a new MCP env value is a secret");
        assert!(!HubSettingsUpdate { machine: Some(Box::new(machine(SecretEdit::Keep))), ..nothing.clone() }.sets_a_secret(), "keeping one is not");
        let save = ClientMessage::SaveSettings { request_id: 1, pairing_key: "k".into(), base_version: "v".into(), update };
        let json = serde_json::to_value(&save).unwrap();
        assert_eq!(json["type"], "saveSettings");
        assert_eq!(json["update"]["providers"][0]["originalId"], "main");
        assert_eq!(json["update"]["providers"][0]["apiKey"], serde_json::json!({ "action": "set", "value": "sk" }));
        assert_eq!(json["update"]["tavilyKey"], serde_json::json!({ "action": "keep" }));
        assert_eq!(json["update"]["whisperKey"], serde_json::json!({ "action": "clear" }));
        assert_eq!(json["update"]["bots"]["telegramAllowedUsers"], serde_json::json!([42]));
        assert_eq!(json["update"]["bots"]["learningMaxPerDay"], 0);
        assert_eq!(json["update"]["telegramToken"], serde_json::json!({ "action": "keep" }));
        assert!(json["update"].get("advanced").is_none() && json["update"].get("machine").is_none(), "absent when untouched");
        assert_eq!(serde_json::from_value::<ClientMessage>(json).unwrap(), save);

        // A save from before P119 carries none of the new fields: it keeps the token and touches nothing else.
        let old: HubSettingsUpdate = serde_json::from_str(r#"{"providers":[],"activeProvider":"","agents":[],"tavilyKey":{"action":"keep"},"whisperKey":{"action":"keep"},"limits":null,"prices":[]}"#).unwrap();
        assert_eq!((old.telegram_token.clone(), old.advanced.is_none(), old.machine.is_none()), (SecretEdit::Keep, true, true));
        // And a hub from before it sends a view without the new slices: they default to empty and locked.
        let machine = MachineSettingsDto::default();
        assert!(!machine.writable && machine.mcp_servers.is_empty() && machine.embedded_server.is_none());
        let edit = MachineEditDto { enable_shell: true, vault_path: "/srv/vault".into(), generated_path: String::new(), mcp_servers: Vec::new(), ssh_hosts: vec![SshHostDto { id: "web".into(), port: 22, ..SshHostDto::default() }], embedded_server: None };
        let json = serde_json::to_value(&edit).unwrap();
        assert_eq!(json["enableShell"], true);
        assert_eq!(json["sshHosts"][0]["requireApproval"], false);
        assert!(json.get("embeddedServer").is_none());
        assert_eq!(serde_json::from_value::<MachineEditDto>(json).unwrap(), edit);

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
            devices: vec![DeviceDto { device_id: "phone".into(), device_name: "Phone".into(), status: DeviceStatusDto::Pending, first_seen_ms: 1, last_seen_ms: 2, user: None }],
            you: "web-1".into(),
        };
        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(json["type"], "deviceList");
        assert_eq!(json["devices"][0], serde_json::json!({ "deviceId": "phone", "deviceName": "Phone", "status": "pending", "firstSeenMs": 1, "lastSeenMs": 2 }));

        let error: ServerMessage = serde_json::from_str(r#"{"type":"deviceError","requestId":4,"message":"m"}"#).unwrap();
        assert_eq!(error, ServerMessage::DeviceError { request_id: 4, message: "m".into(), auth_rejected: false });
    }

    #[test]
    fn api_key_messages_use_the_web_shapes() {
        let create: ClientMessage = serde_json::from_str(r#"{"type":"createApiKey","requestId":2,"pairingKey":"k","name":"n8n"}"#).unwrap();
        assert_eq!(create, ClientMessage::CreateApiKey { request_id: 2, pairing_key: "k".into(), name: "n8n".into(), agent_id: None });
        let bound: ClientMessage = serde_json::from_str(r#"{"type":"createApiKey","requestId":2,"pairingKey":"k","name":"bot","agentId":"poet"}"#).unwrap();
        assert!(matches!(bound, ClientMessage::CreateApiKey { agent_id: Some(ref a), .. } if a == "poet"));
        let revoke: ClientMessage = serde_json::from_str(r#"{"type":"revokeApiKey","requestId":3,"pairingKey":"k","id":"abc"}"#).unwrap();
        assert_eq!(revoke, ClientMessage::RevokeApiKey { request_id: 3, pairing_key: "k".into(), id: "abc".into() });
        let created = ServerMessage::ApiKeyCreated {
            request_id: 2,
            key: "wdn_x".into(),
            keys: vec![ApiKeyDto { id: "abc".into(), name: "n8n".into(), shown: "wdn_12345678".into(), created_at_ms: 5, last_used_at_ms: None, agent_id: None , user: None }],
        };
        assert_eq!(
            serde_json::to_value(&created).unwrap(),
            serde_json::json!({ "type": "apiKeyCreated", "requestId": 2, "key": "wdn_x", "keys": [{ "id": "abc", "name": "n8n", "shown": "wdn_12345678", "createdAtMs": 5 }] })
        );
    }

    #[test]
    fn webhook_messages_use_the_web_shapes() {
        // A webhook sent without `auth` is a token, so a client that doesn't know the field still works.
        let save: ClientMessage = serde_json::from_str(r#"{"type":"saveWebhook","requestId":1,"pairingKey":"k","webhook":{"id":"build","prompt":"why?","enabled":true}}"#).unwrap();
        assert_eq!(
            save,
            ClientMessage::SaveWebhook {
                request_id: 1,
                pairing_key: "k".into(),
                original_id: None,
                webhook: WebhookDto { id: "build".into(), agent_id: None, prompt: "why?".into(), enabled: true, auth: "token".into() },
            }
        );
        let signed: ClientMessage = serde_json::from_str(
            r#"{"type":"saveWebhook","requestId":2,"pairingKey":"k","originalId":"old","webhook":{"id":"gh","agentId":"ops","prompt":"p","enabled":false,"auth":"hmac"}}"#,
        )
        .unwrap();
        assert!(matches!(signed, ClientMessage::SaveWebhook { original_id: Some(ref o), webhook: WebhookDto { agent_id: Some(ref a), enabled: false, ref auth, .. }, .. } if o == "old" && a == "ops" && auth == "hmac"));
        let create: ClientMessage = serde_json::from_str(r#"{"type":"createWebhookCredential","requestId":3,"pairingKey":"k","id":"gh"}"#).unwrap();
        assert_eq!(create, ClientMessage::CreateWebhookCredential { request_id: 3, pairing_key: "k".into(), id: "gh".into() });
        let list: ClientMessage = serde_json::from_str(r#"{"type":"listWebhooks","requestId":4}"#).unwrap();
        assert_eq!(list, ClientMessage::ListWebhooks { request_id: 4 });
        for (json, check) in [
            (r#"{"type":"setWebhookEnabled","requestId":5,"pairingKey":"k","id":"gh","enabled":false}"#, "setWebhookEnabled"),
            (r#"{"type":"deleteWebhook","requestId":6,"pairingKey":"k","id":"gh"}"#, "deleteWebhook"),
            (r#"{"type":"revokeWebhookCredential","requestId":7,"pairingKey":"k","id":"gh"}"#, "revokeWebhookCredential"),
        ] {
            let parsed: ClientMessage = serde_json::from_str(json).unwrap();
            assert_eq!(serde_json::to_value(&parsed).unwrap()["type"], check);
        }

        let info = WebhookInfoDto {
            id: "gh".into(),
            agent_id: None,
            prompt: "p".into(),
            enabled: true,
            auth: "hmac".into(),
            credential: Some("hmac".into()),
            shown: Some("whsec_1234".into()),
            created_at_ms: Some(5),
            last_used_at_ms: None,
            conversation: "task-hook-gh".into(),
        };
        let created = ServerMessage::WebhookCreated { request_id: 3, id: "gh".into(), credential: "whsec_secret".into(), kind: "hmac".into(), webhooks: vec![info.clone()], serves_here: true };
        assert_eq!(
            serde_json::to_value(&created).unwrap(),
            serde_json::json!({
                "type": "webhookCreated", "requestId": 3, "id": "gh", "credential": "whsec_secret", "kind": "hmac", "servesHere": true,
                "webhooks": [{ "id": "gh", "prompt": "p", "enabled": true, "auth": "hmac", "credential": "hmac", "shown": "whsec_1234", "createdAtMs": 5, "conversation": "task-hook-gh" }]
            })
        );
        let listed = ServerMessage::WebhookList { request_id: 4, webhooks: vec![info], serves_here: false };
        let back: ServerMessage = serde_json::from_str(&serde_json::to_string(&listed).unwrap()).unwrap();
        assert_eq!(back, listed);
        let error: ServerMessage = serde_json::from_str(r#"{"type":"webhookError","requestId":1,"message":"nope"}"#).unwrap();
        assert_eq!(error, ServerMessage::WebhookError { request_id: 1, message: "nope".into(), auth_rejected: false });
    }

    #[test]
    fn node_messages_use_the_web_shapes() {
        let hello: ClientMessage = serde_json::from_str(
            r#"{"type":"hello","deviceId":"node-1","deviceName":"Casa","authKey":"k","node":{"description":"PC","tags":["gpu"],"shell":true,"files":false}}"#,
        )
        .unwrap();
        let ClientMessage::Hello { node: Some(offer), .. } = hello else { panic!("{hello:?}") };
        assert_eq!(offer, NodeOfferDto { description: "PC".into(), tags: vec!["gpu".into()], shell: true, files: false, mcp_tools: vec![], models: vec![] });

        let event: ClientMessage = serde_json::from_str(r#"{"type":"modelEvent","requestId":4,"event":{"kind":"content_delta","data":"Olá"}}"#).unwrap();
        assert!(matches!(event, ClientMessage::ModelEvent { request_id: 4, event: StreamEvent::ContentDelta(ref t) } if t == "Olá"));
        let request = ServerMessage::ModelRequest { request_id: 4, model: "ollama".into(), messages: vec![Message::user("oi")], tools: vec![] };
        let back: ServerMessage = serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert!(matches!(back, ServerMessage::ModelRequest { request_id: 4, ref model, ref messages, .. } if model == "ollama" && messages[0].content == "oi"));
        // A client from before P93 sends no `node` and is no node.
        let plain: ClientMessage = serde_json::from_str(r#"{"type":"hello","deviceId":"p","deviceName":"Phone","authKey":"k"}"#).unwrap();
        assert!(matches!(plain, ClientMessage::Hello { node: None, .. }));

        let set: ClientMessage = serde_json::from_str(r#"{"type":"setNodeAccess","requestId":3,"pairingKey":"k","deviceId":"node-1","enabled":true,"agents":["ops"],"requireApproval":true}"#).unwrap();
        assert!(matches!(set, ClientMessage::SetNodeAccess { enabled: true, require_approval: true, ref agents, .. } if agents == &["ops"]));
        let list = ServerMessage::NodeList {
            request_id: 3,
            nodes: vec![NodeInfoDto { device_id: "node-1".into(), name: "Casa".into(), online: true, approved: false, offer: None, enabled: false, agents: vec![], require_approval: false }],
        };
        assert_eq!(
            serde_json::to_value(&list).unwrap(),
            serde_json::json!({ "type": "nodeList", "requestId": 3, "nodes": [{ "deviceId": "node-1", "name": "Casa", "online": true, "approved": false, "enabled": false, "agents": [], "requireApproval": false }] })
        );
    }

    #[test]
    fn task_messages_use_the_web_shapes() {
        let save: ClientMessage = serde_json::from_str(
            r#"{"type":"saveTask","requestId":1,"pairingKey":"k","originalId":"old","task":{"id":"news","agentId":"reader","prompt":"p","cron":"0 8 * * *","enabled":true}}"#,
        )
        .unwrap();
        let ClientMessage::SaveTask { original_id, task, .. } = save else { panic!("{save:?}") };
        assert_eq!(original_id.as_deref(), Some("old"));
        assert_eq!((task.agent_id.as_deref(), task.cron.as_deref(), task.every), (Some("reader"), Some("0 8 * * *"), None));
        let run: ClientMessage = serde_json::from_str(r#"{"type":"runTask","requestId":2,"pairingKey":"k","id":"news"}"#).unwrap();
        assert_eq!(run, ClientMessage::RunTask { request_id: 2, pairing_key: "k".into(), id: "news".into() });
        let enabled: ClientMessage = serde_json::from_str(r#"{"type":"setTaskEnabled","requestId":3,"pairingKey":"k","id":"news","enabled":false}"#).unwrap();
        assert!(matches!(enabled, ClientMessage::SetTaskEnabled { enabled: false, .. }));

        let list = ServerMessage::TaskList {
            request_id: 1,
            runs_here: true,
            tasks: vec![TaskInfoDto {
                task: TaskDto { id: "news".into(), agent_id: None, prompt: "p".into(), every: Some("1h".into()), cron: None, once: None, timezone: None, enabled: true },
                next_run_at_ms: Some(9),
                last_run_at_ms: None,
                last_finished_at_ms: None,
                last_error: None,
                running: false,
                schedule_error: None,
            }],
        };
        assert_eq!(
            serde_json::to_value(&list).unwrap(),
            serde_json::json!({ "type": "taskList", "requestId": 1, "runsHere": true, "tasks": [{ "id": "news", "prompt": "p", "every": "1h", "enabled": true, "nextRunAtMs": 9, "running": false }] })
        );
    }

    #[test]
    fn sync_messages_use_the_web_shapes() {
        let action: ClientMessage = serde_json::from_str(
            r#"{"type":"syncAction","requestId":5,"pairingKey":"k","action":{"kind":"pairJoin","code":"AB12","host":"100.64.0.2"}}"#,
        )
        .unwrap();
        assert_eq!(
            action,
            ClientMessage::SyncAction {
                request_id: 5,
                pairing_key: "k".into(),
                action: SyncActionDto::PairJoin { code: "AB12".into(), host: Some("100.64.0.2".into()) },
            }
        );
        let now: ClientMessage = serde_json::from_str(r#"{"type":"syncAction","requestId":6,"pairingKey":"k","action":{"kind":"syncNow"}}"#).unwrap();
        assert!(matches!(now, ClientMessage::SyncAction { action: SyncActionDto::SyncNow, .. }));

        let status = ServerMessage::SyncStatus {
            request_id: 5,
            status: SyncStatusDto {
                backend: SyncBackendDto::Git,
                git_remote: Some("https://git.example/v.git".into()),
                last_synced_at_ms: None,
                pending_vault_changes: 2,
                pending_config_changed: false,
                last_round: Some(SyncRoundDto { at_ms: 9, pulled: None, pushed: Some(SyncPushedDto { commit_sha: "abc".into(), files_changed: 2 }), error: None }),
                hosting_until_ms: None,
                last_pairing: None,
            },
            pairing_code: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["type"], "syncStatus");
        assert_eq!(
            json["status"],
            serde_json::json!({
                "backend": "git",
                "gitRemote": "https://git.example/v.git",
                "pendingVaultChanges": 2,
                "pendingConfigChanged": false,
                "lastRound": { "atMs": 9, "pushed": { "commitSha": "abc", "filesChanged": 2 } }
            })
        );

        let error: ServerMessage = serde_json::from_str(r#"{"type":"syncError","requestId":5,"message":"m","authRejected":true}"#).unwrap();
        assert_eq!(error, ServerMessage::SyncError { request_id: 5, message: "m".into(), auth_rejected: true });
        assert!(json.get("pairingCode").is_none());
    }

    #[test]
    fn pairing_host_messages_use_the_web_shapes() {
        let host: ClientMessage = serde_json::from_str(r#"{"type":"syncAction","requestId":7,"pairingKey":"k","action":{"kind":"pairHost"}}"#).unwrap();
        assert!(matches!(host, ClientMessage::SyncAction { action: SyncActionDto::PairHost, .. }));
        let cancel: ClientMessage =
            serde_json::from_str(r#"{"type":"syncAction","requestId":8,"pairingKey":"k","action":{"kind":"cancelPairHost"}}"#).unwrap();
        assert!(matches!(cancel, ClientMessage::SyncAction { action: SyncActionDto::CancelPairHost, .. }));

        let status = ServerMessage::SyncStatus {
            request_id: 7,
            status: SyncStatusDto {
                backend: SyncBackendDto::Arweave,
                git_remote: None,
                last_synced_at_ms: None,
                pending_vault_changes: 0,
                pending_config_changed: false,
                last_round: None,
                hosting_until_ms: Some(300_000),
                last_pairing: Some(SyncPairingDto { at_ms: 5, error: Some("cancelled".into()) }),
            },
            pairing_code: Some("AB12CD".into()),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["pairingCode"], "AB12CD");
        assert_eq!(json["status"]["hostingUntilMs"], 300_000);
        assert_eq!(json["status"]["lastPairing"], serde_json::json!({ "atMs": 5, "error": "cancelled" }));
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
