/**
 * Mirrors `crates/warden-server-protocol/src/protocol.rs`. Copied from the extension's
 * `extension/src/protocol/messages.ts` (P78) — keep the two in step when the protocol changes, the
 * same way `mobile/lib/protocol/messages.dart` is kept.
 * Wire shape: internally-tagged JSON with a `type` field, both the tag and every field name
 * camelCase (`#[serde(tag = "type", rename_all = "camelCase", rename_all_fields =
 * "camelCase")]` on the Rust side) — locked by `protocol.rs`'s own round-trip tests, not guessed.
 */

export interface Usage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface Attachment {
  mimeType: string;
  data: string;
}

/** Mirrors `warden_core::tool::ToolSpec` — `parameters` is a raw JSON-Schema object, not a
 * `inputSchema` wrapper. */
export interface ToolSpec {
  name: string;
  description: string;
  parameters: unknown;
}

/** Mirrors `warden_server_protocol::protocol::ProjectDto` (P103). `id` is the folder name and never changes; the files of
 * a project are notes of the vault under `projects/<id>/`. */
export interface ProjectDto {
  id: string;
  name: string;
  description: string;
  instructions: string;
}

/** Mirrors `warden_server_protocol::protocol::SkillDto` (P72). `agents` is the agent restriction
 * (empty = every agent) — this client only displays it; an edit that sends `[]` keeps whatever the
 * server has stored. */
export interface SkillDto {
  name: string;
  description: string;
  body: string;
  agents: string[];
  /** P104 — a suggestion the assistant made after a conversation: nothing in it applies until it's accepted (saved without this). */
  proposed?: boolean;
  /** The conversation a suggestion came from. */
  source?: string;
  /** When it was suggested, in milliseconds since the epoch. */
  proposedAt?: number;
  /** P115: the suggestion is a change to this existing skill; accepting applies it there. */
  revises?: string;
}

/** Mirrors `warden_server_protocol::protocol::ConversationSummary` (P78). */
export interface ConversationSummary {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  /** The agent (P46) this conversation last spoke with — absent for none. */
  agentId?: string;
  /** The project (P103) the conversation was started in — fixed for its life; absent for none (or a hub from before projects). */
  projectId?: string;
}

/** Mirrors `warden_server_protocol::protocol::HistoryMessage` (P40). */
export interface HistoryMessage {
  role: "user" | "assistant";
  content: string;
  createdAt: number;
  attachments: Attachment[];
}

/** Mirrors `warden_server_protocol::protocol::VaultSearchHit` (P78). `lineNumber` is 1-based. */
export interface VaultSearchHit {
  path: string;
  lineNumber: number;
  line: string;
}

/** Mirrors `warden_server_protocol::protocol::LimitStatusDto` (P78/P4). `fraction` >= 1 is exhausted;
 * `extendTokens`/`extendCostUsd` are what one `extendLimit` adds. */
export interface LimitStatus {
  id: string;
  scope: string;
  windowHours: number;
  usedTokens: number;
  maxTokens: number | null;
  usedCostUsd: number;
  maxCostUsd: number | null;
  fraction: number;
  warn: boolean;
  exceeded: boolean;
  unpricedCalls: number;
  freesUpInMinutes: number | null;
  extendTokens: number;
  extendCostUsd: number;
}

export interface SpendBucket {
  key: string;
  calls: number;
  tokens: number;
  costUsd: number;
  unpricedCalls: number;
}

/** Mirrors `warden_server_protocol::protocol::UsageReportDto` (P78). */
export interface UsageReport {
  total: Usage;
  conversationCount: number;
  messageCount: number;
  byDevice: { deviceId: string; name?: string; conversationCount: number; messageCount: number; usage: Usage }[];
  daily: { date: string; calls: number; tokens: number }[];
  /** P10 — dollars per day from the ledger, which only reaches back `recent.windowHours`. Absent from an older hub. */
  dailyCost?: { date: string; calls: number; costUsd: number; unpricedCalls: number }[];
  limitsEnabled: boolean;
  limits: LimitStatus[];
  recent?: RecentSpend;
  ledgerError?: string;
}

/** Mirrors `RecentSpendDto`: what the ledger still holds. `byProvider`, `byAgent` and `byPerson` (P10) are absent
 * from an older hub; an empty `key` is a call with none of them (the owner, or from before it was kept). */
export interface RecentSpend {
  windowHours: number;
  byModel: SpendBucket[];
  byChannel: SpendBucket[];
  byProvider?: SpendBucket[];
  byAgent?: SpendBucket[];
  byPerson?: SpendBucket[];
}

/** Mirrors `SecretStatusDto` (P78): whether an API key is saved, never the key itself. `hint` is its
 * last four characters, only for a key long enough that they give nothing away. */
export interface SecretStatus {
  set: boolean;
  hint?: string;
}

/** Mirrors `SecretEdit`: what a save does to one secret. `keep` is an untouched field. */
export type SecretEdit = { action: "keep" } | { action: "set"; value: string } | { action: "clear" };

export type ProviderKind = "gemini" | "openai" | "anthropic" | "openai_compatible" | "node";

/** Mirrors `ProviderSettingsDto`. Empty strings mean "not set". */
export interface ProviderSettings {
  id: string;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  apiKey: SecretStatus;
  /** Kind "node" only (P93): the node's device id. */
  node?: string;
}

/** Mirrors `ProviderEditDto`. `originalId` is the id it had when loaded (absent for a new one), so a
 * renamed provider keeps its saved key under `keep`. */
export interface ProviderEdit {
  originalId?: string;
  id: string;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  apiKey: SecretEdit;
  /** Kind "node" only (P93): the node's device id. */
  node: string;
}

/** Mirrors `AgentSettingsDto`. `providerId` is empty for "no default model"; `allowedTools: null`
 * keeps every tool. `originalId` carries a rename on save. */
export interface AgentSettings {
  originalId?: string;
  id: string;
  persona: string;
  providerId: string;
  canDelegateToAgents: boolean;
  canManageAgents: boolean;
  canMessageAgents: boolean;
  /** P92 — the `manage_tasks` tool. */
  canManageTasks: boolean;
  allowedTools: string[] | null;
  /** P84 — members this agent is shared with, or ["*"] for everyone. */
  sharedWith?: string[];
  /** P84 — in a member's view, their username on their own agents; absent on the shared ones. */
  owner?: string;
}

/** P84 — `person` is one workspace member, on every channel. */
export type LimitScope = "global" | "agent" | "channel" | "user" | "person";

/** Mirrors `LimitSettingsDto` (P4). `target` is empty for a global limit; `warnAt`/`extendStep` are
 * fractions (0–1), `null` for the default. */
export interface LimitSettings {
  id: string;
  scope: LimitScope;
  target: string;
  windowHours: number;
  maxTokens: number | null;
  maxCostUsd: number | null;
  warnAt: number | null;
  extendStep: number | null;
}

export interface PriceSettings {
  model: string;
  inputPerMtok: number;
  outputPerMtok: number;
}

/** Mirrors `HubSettingsDto` (P78): the part of the hub's config the web edits. */
export interface HubSettings {
  providers: ProviderSettings[];
  activeProvider: string;
  /** Named routing combos (P90): picked like a provider; tries its providers in order when one is down. */
  combos: Combo[];
  agents: AgentSettings[];
  tavilyKey: SecretStatus;
  whisperKey: SecretStatus;
  /** `null`: no limits written, the built-in ones apply. `[]`: every limit off. */
  limits: LimitSettings[] | null;
  defaultLimits: LimitSettings[];
  limitsDisabledByEnv: boolean;
  prices: PriceSettings[];
  defaultModels: Record<string, string>;
  toolNames: string[];
  /** `[git_sync]` (P61): `remoteUrl` empty = no git sync; the token is never sent. */
  gitSync: { remoteUrl: string; token: SecretStatus };
  /** `[learning]` and the bots' allow-lists (P118). */
  bots: BotsSettings;
  /** The Telegram bot's token (P119): whether one is saved, never the token. */
  telegramToken: SecretStatus;
  /** Delegation ceilings and TruthID (P119). */
  advanced: AdvancedSettings;
  /** What reaches the hub's own machine (P119): read-only unless `machine.writable`. */
  machine: MachineSettings;
  notes: string[];
}

/** Mirrors `AdvancedSettingsDto` (P119). A `null` ceiling keeps the hub's built-in one. */
export interface AdvancedSettings {
  delegateMaxDepth: number | null;
  maxDelegatedCalls: number | null;
  maxParallelJobs: number | null;
  /** `base-mainnet` or `base-sepolia`. */
  truthidNetwork: string;
  truthidRpcUrl: string;
  truthidPublicUrl: string;
}

/** Mirrors `McpServerSettingsDto`: the names of the secret values, never the values. */
export interface McpServerSettings {
  name: string;
  kind: "stdio" | "http";
  command: string;
  args: string[];
  envKeys: string[];
  url: string;
  headerKeys: string[];
  /** Signs in with OAuth, which only the desktop can run. */
  oauth: boolean;
}

/** Mirrors `SshHostDto`: only the path to a key, never the key. */
export interface SshHost {
  id: string;
  host: string;
  user: string;
  port: number;
  identityFile: string;
  enabled: boolean;
  agents: string[];
  requireApproval: boolean;
}

/** Mirrors `EmbeddedServerDto`: the desktop's embedded hub, without its key. Counts from its next start. */
export interface EmbeddedServer {
  enabled: boolean;
  port: number;
  listenHost: string;
  serverName: string;
  tailscaleCert: boolean;
  tlsCert: string;
  tlsKey: string;
  tlsHost: string;
  webUi: boolean;
}

/** Mirrors `MachineSettingsDto` (P119). `writable` is false unless the hub was started with
 * `--allow-machine-settings` and this connection is encrypted or local; `blockedReason` says which. */
export interface MachineSettings {
  writable: boolean;
  blockedReason: string;
  enableShell: boolean;
  vaultPath: string;
  generatedPath: string;
  mcpServers: McpServerSettings[];
  sshHosts: SshHost[];
  /** `null`: the hub has none; it is set up on the desktop. */
  embeddedServer: EmbeddedServer | null;
}

/** Mirrors `SecretEntryEdit`: one env entry or header of an MCP server. */
export interface SecretEntryEdit {
  key: string;
  value: SecretEdit;
}

/** Mirrors `McpServerEditDto`. `originalName` finds the saved server, so `keep` carries its values over. */
export interface McpServerEdit {
  originalName?: string;
  name: string;
  kind: "stdio" | "http";
  command: string;
  args: string[];
  env: SecretEntryEdit[];
  url: string;
  headers: SecretEntryEdit[];
}

/** Mirrors `MachineEditDto`. */
export interface MachineEdit {
  enableShell: boolean;
  vaultPath: string;
  generatedPath: string;
  mcpServers: McpServerEdit[];
  sshHosts: SshHost[];
  /** Omitted: the embedded hub stays as it is. The web never creates one. */
  embeddedServer?: EmbeddedServer;
}

/** Mirrors `BotsSettingsDto`: learning on or off and the lists of who may talk to the bots. Empty lists mean nobody. */
export interface BotsSettings {
  learningEnabled: boolean;
  /** A provider or combo id; empty is the active model. */
  learningProvider: string;
  learningMaxPerDay: number;
  /** `telegram:<id>` / `whatsapp:<id>`. */
  learningBotChats: string[];
  telegramAllowedUsers: number[];
  whatsappAllowedChats: string[];
  /** A stranger who writes to the bot gets a code for the owner to approve (P117). */
  telegramPairing: boolean;
  whatsappPairing: boolean;
}

/** Mirrors `BotPairingDto` (P117): a stranger waiting for the owner to let them talk to a bot. */
export interface BotPairing {
  channel: "telegram" | "whatsapp";
  /** The Telegram user id or the WhatsApp chat id that would go on the allow-list. */
  sender: string;
  label: string;
  /** As the sender was told: `ABCD-EFGH`. */
  code: string;
  /** Unix seconds. */
  expiresAt: number;
}

/** Mirrors `BotMemberDto` (P117): a member of the workspace an approved chat may speak as, and whether the bots
 * are linked to the hub as them (`warden bots link`); the hub refuses to approve a chat as someone who isn't. */
export interface BotMember {
  id: string;
  name: string;
  linked: boolean;
}

/** What the pairing screen shows: who is waiting, and who a chat may be approved as speaking as. */
export interface BotPairingsView {
  pairings: BotPairing[];
  members: BotMember[];
}

/** Mirrors `DeviceDto`: one device in the hub's pairing registry, never its token. */
export interface HubDevice {
  deviceId: string;
  deviceName: string;
  status: "pending" | "approved" | "revoked";
  firstSeenMs: number;
  lastSeenMs: number;
  /** P84: the member it belongs to; absent for the owner's. */
  user?: string;
}

/** Mirrors `UserInfoDto` (P84): a member of the workspace, never the password or its hash. */
export interface UserInfo {
  /** The username. */
  id: string;
  name: string;
  role: string;
  /** Still on the provisional password: the hub only accepts `changePassword` until it's changed. */
  mustChangePassword: boolean;
  /** P84 fatia 2 — the tools the owner set for them; absent/null is the safe default. */
  tools?: string[] | null;
  /** Their own agents' names. */
  agents: string[];
  /** P84 fatia 4 — their data is encrypted on the hub with a key only they (or their recovery code) can open. */
  encrypted?: boolean;
  /** The owner reset their password: the data opens only with the recovery code, given when they pick a new password. */
  needsRecovery?: boolean;
  /** Only in `helloAck`: the hub doesn't hold their key (it restarted), so their data is shut until they sign in with the password. */
  locked?: boolean;
  /** P84 fatia 4 parte B — who besides them may open their data right now: "private", "consent" or "company"; absent while it isn't encrypted. */
  memberPolicy?: string;
  /** The workspace's policy is a weaker one they haven't accepted yet (or needs a new recovery code this client couldn't show). */
  policyPending?: boolean;
  /** Only in `helloAck`: the workspace's recovery policy. */
  recoveryPolicy?: string;
  /** P115 — the member turned off the assistant learning from their conversations. */
  learningOptOut?: boolean;
  /** P115 — the provider or combo the owner chose for learning from their conversations; absent: the workspace's. */
  learningProvider?: string;
  /** Only in `helloAck`: the workspace has learning on at all, so the member's switch means something. */
  learningEnabled?: boolean;
  /** Every time the owner recovered their data with the workspace's recovery key. */
  recoveries?: RecoveryEvent[];
  /** P84 fatia 5 — the TruthID username they linked; absent if none. */
  truthid?: string;
  /** An invite to link a TruthID is open (not used or expired yet). */
  inviteOpen?: boolean;
}

/** Mirrors `RemovedUserDto` (P84 fatia 4): removed, with their encrypted data still on the hub's disk. */
export interface RemovedUser {
  id: string;
  name: string;
}

/** Mirrors `RecoveryEventDto` (P84 fatia 4 parte B). */
export interface RecoveryEvent {
  /** Milliseconds since the epoch. */
  atMs: number;
  /** The policy it was done under: "consent" or "company". */
  kind: string;
  /** The person has seen it. */
  seen: boolean;
}

/** P84 fatia 3 — a folder of the owner's vault shared with members, who see it at
 * `compartilhado/<id>/` in their own vault. To a member, `folder` is that path. */
export interface SpaceInfo {
  id: string;
  folder: string;
  /** Usernames, or `"*"` for everyone. */
  readers: string[];
  /** Writers also read. */
  writers: string[];
}

/** Mirrors `ApiKeyDto` (P12): one Warden API key, never the key or its hash. `shown` is its start. */
export interface ApiKey {
  id: string;
  name: string;
  shown: string;
  createdAtMs: number;
  lastUsedAtMs?: number;
  /** The only agent this key speaks as; absent for a general key. */
  agentId?: string;
  /** P84 — the member it belongs to; absent for the owner's. */
  user?: string;
}

/** Mirrors `NodeOfferDto` (P93): what a node lends, as its operator chose. */
export interface NodeOffer {
  description: string;
  tags: string[];
  shell: boolean;
  files: boolean;
  /** Tools of the MCP servers it lends; the agents see each as `<node>__<tool>`. */
  mcpTools?: { name: string; description: string }[];
  /** Model providers it lends, by their id on the node (a hub provider of kind "node" uses one). */
  models?: string[];
}

/** Mirrors `NodeInfoDto`: a node, what it offers and what the hub lets agents do with it. */
export interface NodeInfo {
  deviceId: string;
  name: string;
  online: boolean;
  /** Approved in the device list — needed before any agent can use it. */
  approved: boolean;
  offer?: NodeOffer;
  enabled: boolean;
  /** Empty = every agent. */
  agents: string[];
  requireApproval: boolean;
}

/** Mirrors `TaskDto` (P92): one scheduled task. Exactly one of `every`, `cron` and `once`. */
export interface Task {
  id: string;
  /** The agent that runs it; absent runs with no persona. */
  agentId?: string;
  prompt: string;
  every?: string;
  cron?: string;
  once?: string;
  timezone?: string;
  enabled: boolean;
}

/** Mirrors `TaskInfoDto`: a task and where it stands on the hub. */
export interface TaskInfo extends Task {
  /** Absent when paused, done (a `once` that ran) or the schedule is invalid. */
  nextRunAtMs?: number;
  lastRunAtMs?: number;
  lastFinishedAtMs?: number;
  lastError?: string;
  running: boolean;
  scheduleError?: string;
}

/** Mirrors `HubSettingsUpdate`: replaces the editable part, the rest of the file is kept. */
export interface HubSettingsUpdate {
  providers: ProviderEdit[];
  activeProvider: string;
  agents: AgentSettings[];
  tavilyKey: SecretEdit;
  whisperKey: SecretEdit;
  limits: LimitSettings[] | null;
  prices: PriceSettings[];
  /** Omitted: the combos stay, minus any provider this save removed. */
  combos?: Combo[];
  /** Omitted: `[git_sync]` stays as it is. An empty `remoteUrl` turns git sync off. */
  gitSync?: { remoteUrl: string; token: SecretEdit };
  /** Omitted: `[learning]` and the bots' lists stay as they are. */
  bots?: BotsSettings;
  /** What to do with the Telegram bot's token (P119). */
  telegramToken?: SecretEdit;
  /** Omitted: the delegation ceilings and TruthID stay as they are. */
  advanced?: AdvancedSettings;
  /** Omitted: everything that reaches the hub's machine stays as it is. The hub refuses a save that has it
   * unless it was started with `--allow-machine-settings` and the connection is encrypted or local. */
  machine?: MachineEdit;
}

/** Mirrors `ComboDto` (P90): provider ids, in order. Its id shares one namespace with the providers'. */
export interface Combo {
  id: string;
  providers: string[];
}

/** Mirrors `ProviderFallbackDto` (P79): `from` failed with `reason`, `to` answered with `model`. */
export interface ProviderFallback {
  from: string;
  to: string;
  model: string;
  reason: string;
}

/** Mirrors `SyncRoundDto` (P61): one round the hub ran; `pulled`/`pushed` only when something moved. */
export interface SyncRound {
  atMs: number;
  pulled?: { filesWritten: number; filesDeleted: number; configUpdated: boolean };
  pushed?: { commitSha: string; filesChanged: number };
  error?: string;
}

/** Mirrors `SyncStatusDto`: where the hub's vault syncs to and how the last round went. */
export interface SyncStatus {
  backend: "notSetUp" | "git" | "arweave";
  gitRemote?: string;
  lastSyncedAtMs?: number;
  pendingVaultChanges: number;
  pendingConfigChanged: boolean;
  lastRound?: SyncRound;
  /** Until when the hub is showing a pairing code (P88). The code itself only comes back to `pairHost`. */
  hostingUntilMs?: number;
  /** How the last pairing the hub showed a code for ended; no `error` means a device joined. */
  lastPairing?: { atMs: number; error?: string };
}

/** Mirrors `SyncActionDto`. `host` is an IPv4 address (a Tailscale one works). */
export type SyncAction =
  | { kind: "syncNow" }
  | { kind: "init" }
  | { kind: "pairJoin"; code: string; host?: string }
  | { kind: "pairHost" }
  | { kind: "cancelPairHost" };

export type ClientMessage =
  /** `authKey` is the hub's pairing key (P36), only needed until this device holds a
   * `deviceToken` from an earlier `helloAck`. */
  | { type: "hello"; deviceId: string; deviceName: string; authKey: string; deviceToken?: string; tools: ToolSpec[]; username?: string; password?: string; recoveryCodes?: boolean }
  | { type: "ping"; nonce: number }
  /** `conversationId` (P78) picks one of this device's conversations — a new id starts a new one;
   * omitted, the turn goes to the device's default conversation. */
  | { type: "chat"; message: string; conversationId?: string; attachments?: Attachment[]; agentId?: string; projectId?: string }
  /** P46 — the person's answer to an `approvalRequest`. */
  | { type: "resolveApproval"; approvalId: number; approved: boolean }
  | { type: "toolCallResult"; callId: number; result: unknown }
  | { type: "toolCallError"; callId: number; message: string }
  /** Skills management (P72) — `requestId` is echoed on the matching reply. */
  | { type: "listSkills"; requestId: number }
  | { type: "saveSkill"; requestId: number; skill: SkillDto; overwrite: boolean }
  | { type: "deleteSkill"; requestId: number; name: string }
  /** Projects (P103) — of the person's own vault; `requestId` is echoed on the matching reply. */
  | { type: "listProjects"; requestId: number }
  | { type: "saveProject"; requestId: number; project: ProjectDto; overwrite: boolean }
  | { type: "deleteProject"; requestId: number; id: string }
  /** P40 — this device's persisted conversation, answered by `history`/`historyError` with the same
   * `requestId`. `limit` keeps only the most recent messages. */
  | { type: "requestHistory"; requestId: number; limit?: number; conversationId?: string }
  /** P78 — this device's conversations, answered by `conversationList`/`conversationOk`/`conversationError`. */
  | { type: "listConversations"; requestId: number }
  | { type: "renameConversation"; requestId: number; conversationId: string; title: string }
  | { type: "deleteConversation"; requestId: number; conversationId: string }
  /** P78 — voice input: the hub transcribes the recording (Whisper) and answers with
   * `transcription`/`transcriptionError`. */
  | { type: "transcribe"; requestId: number; audio: Attachment }
  /** P78 — the hub's vault. A save without `expectedVersion` creates the note; with it, the hub
   * refuses (`vaultError` with `conflict: true`) if the note changed since that version was read. */
  | { type: "listVaultFiles"; requestId: number }
  | { type: "readVaultNote"; requestId: number; path: string }
  | { type: "saveVaultNote"; requestId: number; path: string; content: string; expectedVersion?: string }
  | { type: "deleteVaultNote"; requestId: number; path: string; expectedVersion: string }
  | { type: "searchVault"; requestId: number; query: string }
  /** P78 — usage across the whole hub; `tzOffsetMinutes` is the viewer's offset from UTC (UTC−3 = -180). */
  | { type: "requestUsage"; requestId: number; tzOffsetMinutes: number }
  /** P78 — one `extend_step` more for a spending limit, answered by `limitExtended`. */
  | { type: "extendLimit"; requestId: number; limitId: string }
  /** P78 — the hub's settings. A save repeats the pairing key and sends the `version` it loaded. */
  | { type: "requestSettings"; requestId: number }
  | { type: "saveSettings"; requestId: number; pairingKey: string; baseVersion: string; update: HubSettingsUpdate }
  /** Sessão 103 — the hub's paired devices; approving or revoking repeats the pairing key. */
  | { type: "listDevices"; requestId: number }
  | { type: "listApiKeys"; requestId: number }
  | { type: "createApiKey"; requestId: number; pairingKey: string; name: string; agentId?: string }
  | { type: "revokeApiKey"; requestId: number; pairingKey: string; id: string }
  /** P93 — nodes; changing one's access repeats the pairing key. */
  | { type: "listNodes"; requestId: number }
  | { type: "setNodeAccess"; requestId: number; pairingKey: string; deviceId: string; enabled: boolean; agents: string[]; requireApproval: boolean }
  /** P92 — scheduled tasks; every change repeats the pairing key. */
  | { type: "listTasks"; requestId: number }
  | { type: "saveTask"; requestId: number; pairingKey: string; originalId?: string; task: Task }
  | { type: "setTaskEnabled"; requestId: number; pairingKey: string; id: string; enabled: boolean }
  | { type: "deleteTask"; requestId: number; pairingKey: string; id: string }
  | { type: "runTask"; requestId: number; pairingKey: string; id: string }
  | { type: "setDeviceStatus"; requestId: number; pairingKey: string; deviceId: string; action: "approve" | "revoke" }
  /** P61 — the hub's vault sync; an action repeats the pairing key. */
  | { type: "requestSyncStatus"; requestId: number }
  | { type: "syncAction"; requestId: number; pairingKey: string; action: SyncAction }
  /** P84 — a member picks their own password; the owner manages the members with the pairing key. */
  | { type: "changePassword"; requestId: number; oldPassword: string; newPassword: string; recoveryCode?: string }
  /** P84 fatia 4 — a new recovery code, with the password; the old one stops working. */
  | { type: "regenerateRecoveryCode"; requestId: number; password: string }
  /** P84 fatia 4 parte B — a member says yes to a weaker recovery policy, with their password; the owner sets the
   * policy and recovers a member with the workspace's recovery key (`consent` also needs the person's `code`). */
  | { type: "acceptRecoveryPolicy"; requestId: number; password: string }
  | { type: "ackRecoveryNotices"; requestId: number }
  | { type: "setLearning"; requestId: number; enabled: boolean }
  | { type: "setRecoveryPolicy"; requestId: number; pairingKey: string; policy: string; newKey: boolean }
  | { type: "recoverMember"; requestId: number; pairingKey: string; id: string; recoveryKey: string; code?: string }
  | { type: "listUsers"; requestId: number }
  | { type: "saveUser"; requestId: number; pairingKey: string; id: string; name: string; isNew: boolean }
  | { type: "resetPassword"; requestId: number; pairingKey: string; id: string }
  | { type: "removeUser"; requestId: number; pairingKey: string; id: string }
  /** P84 fatia 4 — the owner brings back a removed member whose encrypted data was kept. */
  | { type: "restoreUser"; requestId: number; pairingKey: string; id: string }
  /** P84 fatia 5 — the owner makes an invite to link a TruthID (shown once, in `userList`) or unties one; a member
   * links theirs with the invite `code` and their TruthID `username`. */
  | { type: "createInvite"; requestId: number; pairingKey: string; id: string }
  | { type: "unlinkTruthId"; requestId: number; pairingKey: string; id: string }
  | { type: "redeemInvite"; requestId: number; code: string; username: string }
  /** P84 fatia 2 — the owner sets a member's tools (`null`: the safe default). */
  | { type: "setUserTools"; requestId: number; pairingKey: string; id: string; tools: string[] | null }
  | { type: "setUserLearningProvider"; requestId: number; pairingKey: string; id: string; provider: string | null }
  /** A member's own agents, answered by `settings` (their view) or `settingsError`. */
  | { type: "saveOwnAgent"; requestId: number; originalId?: string; agent: AgentSettings }
  | { type: "deleteOwnAgent"; requestId: number; id: string }
  /** P84 fatia 3 — the shared spaces; the owner changes them with the pairing key. */
  | { type: "listSpaces"; requestId: number }
  | { type: "saveSpace"; requestId: number; pairingKey: string; originalId?: string; space: SpaceInfo }
  | { type: "deleteSpace"; requestId: number; pairingKey: string; id: string }
  /** P117 — the owner answers the bots' pairing requests; deciding needs the pairing key. */
  | { type: "listBotPairings"; requestId: number }
  /** `member`: approve the chat as speaking as that member of the workspace; absent, as the owner. */
  | { type: "resolveBotPairing"; requestId: number; pairingKey: string; code: string; approve: boolean; member?: string }
  /** P10 — checks a provider's key without spending a conversation. `provider` is the form: `keep` on its key uses the
   * saved one (found by `originalId`), `set` is a key typed but not saved. Answered by `providerTest` or `userError`. */
  | { type: "testProvider"; requestId: number; pairingKey: string; provider: ProviderEdit }
  /** Fase 9.1 (redefined) — an unauthenticated presence probe, answered by `discoverAck` below.
   * No `authKey`/`deviceId` on purpose: the point is finding a hub before knowing its credential. */
  | { type: "discover" }
  | { type: "goodbye"; reason: string | null };

export function encode(message: ClientMessage): string {
  return JSON.stringify(message);
}

export type ServerMessage =
  /** `deviceToken` is present when this Hello paired with the pairing key (P36) — it replaces
   * whatever token this device held for the hub. */
  | { type: "helloAck"; serverName: string; deviceToken?: string; user?: UserInfo }
  | { type: "authError"; reason: string }
  | { type: "pong"; nonce: number }
  /** `conversationId` (P78) — which conversation this answers; `chat` has no `requestId`. */
  /** `fallbacks` (P79): the turn's provider was down and a reserve answered — empty almost always. */
  | { type: "chatResponse"; content: string; usage: Usage | null; attachments: Attachment[]; conversationId?: string; fallbacks: ProviderFallback[] }
  /** `spendLimitId` (P4/P78): the turn stopped on that spending limit — offer `extendLimit`. */
  | { type: "chatError"; message: string; conversationId?: string; spendLimitId?: string }
  | { type: "toolCallRequest"; callId: number; tool: string; arguments: unknown }
  | { type: "skillList"; requestId: number; skills: SkillDto[] }
  | { type: "skillOk"; requestId: number }
  | { type: "skillError"; requestId: number; message: string }
  | { type: "projectList"; requestId: number; projects: ProjectDto[] }
  | { type: "projectOk"; requestId: number }
  | { type: "projectError"; requestId: number; message: string }
  | { type: "history"; requestId: number; messages: HistoryMessage[] }
  | { type: "historyError"; requestId: number; message: string }
  | { type: "conversationList"; requestId: number; conversations: ConversationSummary[] }
  | { type: "conversationOk"; requestId: number }
  | { type: "conversationError"; requestId: number; message: string }
  | { type: "transcription"; requestId: number; text: string }
  | { type: "transcriptionError"; requestId: number; message: string }
  | { type: "vaultFileList"; requestId: number; files: string[] }
  | { type: "vaultNote"; requestId: number; path: string; content: string; version: string }
  | { type: "vaultSaved"; requestId: number; version: string }
  | { type: "vaultOk"; requestId: number }
  | { type: "vaultSearchResults"; requestId: number; hits: VaultSearchHit[] }
  | { type: "vaultError"; requestId: number; message: string; conflict: boolean }
  | { type: "usageReport"; requestId: number; report: UsageReport }
  | { type: "limitExtended"; requestId: number; limit: LimitStatus }
  | { type: "usageError"; requestId: number; message: string }
  /** `secretsWritable` is false on plain http:// from another machine, where the hub refuses a new key. */
  | { type: "settings"; requestId: number; settings: HubSettings; version: string; secretsWritable: boolean }
  | { type: "settingsSaved"; requestId: number; settings: HubSettings; version: string }
  | { type: "settingsError"; requestId: number; message: string; conflict: boolean; authRejected: boolean }
  /** `you` is this browser's own device id. */
  | { type: "deviceList"; requestId: number; devices: HubDevice[]; you: string }
  | { type: "deviceError"; requestId: number; message: string; authRejected: boolean }
  | { type: "apiKeyList"; requestId: number; keys: ApiKey[] }
  | { type: "apiKeyCreated"; requestId: number; key: string; keys: ApiKey[] }
  | { type: "apiKeyError"; requestId: number; message: string; authRejected: boolean }
  | { type: "nodeList"; requestId: number; nodes: NodeInfo[] }
  | { type: "nodeError"; requestId: number; message: string; authRejected: boolean }
  /** `runsHere`: this hub runs the tasks on schedule. */
  | { type: "taskList"; requestId: number; tasks: TaskInfo[]; runsHere: boolean }
  | { type: "taskError"; requestId: number; message: string; authRejected: boolean }
  | { type: "syncStatus"; requestId: number; status: SyncStatus; pairingCode?: string }
  | { type: "syncError"; requestId: number; message: string; authRejected: boolean }
  /** P84 — `tempPassword`: the provisional password of the member just created or reset, shown once. */
  | { type: "userList"; requestId: number; users: UserInfo[]; tempPassword?: string; inviteCode?: string; recoveryPolicy?: string; removed?: RemovedUser[] }
  /** P84 fatia 5 (P113) — the answer to a `hello` with `truthidLogin`: the QR's JSON for the TruthID app, good until
   * `expiresAtMs`. The `helloAck` follows once the phone approves. */
  | { type: "truthIdChallenge"; payload: string; expiresAtMs: number }
  /** P84 fatia 5 — the member's TruthID is linked. */
  | { type: "truthIdLinked"; requestId: number; username: string }
  /** `secret`: the owner's recovery key, only when one was just made — shown once, never kept. */
  | { type: "recoveryPolicy"; requestId: number; policy: string; secret?: string }
  /** `recoveryCode`: entering or leaving "consent" made a new code — shown once. */
  | { type: "recoveryPolicyAccepted"; requestId: number; recoveryCode?: string }
  | { type: "recoveryNoticesAcked"; requestId: number }
  | { type: "learningSet"; requestId: number }
  /** P117 — the strangers waiting for the owner to let them talk to a bot, oldest first. */
  | { type: "botPairings"; requestId: number; pairings: BotPairing[]; members?: BotMember[] }
  /** P10 — what testing a provider's key came to: `ok` only when the provider accepted it. `kind` is `ok`, `unverifiable`,
   * `rejected`, `rate_limited`, `provider_down`, `unreachable` or `unsupported`; `message` carries neither the key nor what the provider said. */
  | { type: "providerTest"; requestId: number; ok: boolean; kind: string; message: string }
  /** `recoveryCode`: this change turned encryption on for their data — shown once, they have to write it down. */
  | { type: "passwordChanged"; requestId: number; recoveryCode?: string }
  /** A recovery code, shown once: the answer to `regenerateRecoveryCode`, or (`requestId` 0) sent right after
   * `helloAck` when signing in turned encryption on for a member from before. */
  | { type: "recoveryCode"; requestId: number; code: string }
  | { type: "spaceList"; requestId: number; spaces: SpaceInfo[] }
  | { type: "userError"; requestId: number; message: string; authRejected: boolean }
  /** P46 — a tool in this browser's chat turn needs the person's yes; answer with `resolveApproval`. */
  | { type: "approvalRequest"; approvalId: number; target: string; action: string; detail: string }
  /** The hub stopped waiting (deadline): close the prompt. */
  | { type: "approvalCancelled"; approvalId: number }
  /** An agent left a message for another, or answered one, in one of this browser's conversations. */
  | { type: "conversationsChanged"; conversationId: string }
  /** Reply to `ClientMessage.discover` — just enough to let the operator recognize which machine
   * this is, never a secret. */
  /** `secureUrl` — set by a TLS-only hub (P36): the wss:// URL to connect to instead. */
  | { type: "discoverAck"; serverName: string; secureUrl?: string }
  | { type: "goodbye"; reason: string | null };

/**
 * Decodes one `ServerMessage`. Throws on anything this client doesn't understand — explicit is
 * better than a silent `as` cast producing a message shape this client can't actually handle.
 */
export function decode(text: string): ServerMessage {
  const json = JSON.parse(text) as { type?: unknown };
  switch (json.type) {
    case "helloAck":
    case "authError":
    case "pong":
    case "chatError":
    case "discoverAck":
    case "goodbye":
      return json as ServerMessage;
    case "chatResponse": {
      const raw = json as { content: string; usage: Usage | null; attachments?: Attachment[]; conversationId?: string; fallbacks?: ProviderFallback[] };
      return {
        type: "chatResponse",
        content: raw.content,
        usage: raw.usage,
        attachments: raw.attachments ?? [],
        conversationId: raw.conversationId,
        fallbacks: raw.fallbacks ?? [],
      };
    }
    case "skillList": {
      const raw = json as { requestId: number; skills: Array<Omit<SkillDto, "agents"> & { agents?: string[] }> };
      return { type: "skillList", requestId: raw.requestId, skills: raw.skills.map((skill) => ({ ...skill, agents: skill.agents ?? [] })) };
    }
    case "skillOk":
    case "skillError":
    case "projectList":
    case "projectOk":
    case "projectError":
    case "historyError":
    case "conversationList":
    case "conversationOk":
    case "conversationError":
    case "transcription":
    case "transcriptionError":
    case "vaultFileList":
    case "vaultNote":
    case "vaultSaved":
    case "vaultOk":
    case "vaultSearchResults":
    case "usageReport":
    case "limitExtended":
    case "usageError":
    case "settings":
    case "settingsSaved":
    case "deviceList":
    case "apiKeyList":
    case "apiKeyCreated":
    case "syncStatus":
    case "approvalRequest":
    case "approvalCancelled":
    case "conversationsChanged":
    case "passwordChanged":
    case "recoveryCode":
    case "recoveryPolicy":
    case "recoveryPolicyAccepted":
    case "recoveryNoticesAcked":
    case "learningSet":
    case "botPairings":
    case "providerTest":
    case "truthIdLinked":
    case "truthIdChallenge":
      return json as ServerMessage;
    case "userList": {
      const raw = json as { requestId: number; users: Array<Omit<UserInfo, "mustChangePassword" | "agents"> & { mustChangePassword?: boolean; agents?: string[] }>; tempPassword?: string; inviteCode?: string; recoveryPolicy?: string; removed?: RemovedUser[] };
      return {
        type: "userList",
        requestId: raw.requestId,
        users: raw.users.map((u) => ({ ...u, mustChangePassword: u.mustChangePassword ?? false, agents: u.agents ?? [] })),
        ...(raw.tempPassword !== undefined && { tempPassword: raw.tempPassword }),
        ...(raw.inviteCode !== undefined && { inviteCode: raw.inviteCode }),
        removed: raw.removed ?? [],
        ...(raw.recoveryPolicy !== undefined && { recoveryPolicy: raw.recoveryPolicy }),
      };
    }
    case "spaceList": {
      const raw = json as { requestId: number; spaces: Array<Omit<SpaceInfo, "readers" | "writers"> & { readers?: string[]; writers?: string[] }> };
      return { type: "spaceList", requestId: raw.requestId, spaces: raw.spaces.map((s) => ({ ...s, readers: s.readers ?? [], writers: s.writers ?? [] })) };
    }
    case "userError": {
      const raw = json as { requestId: number; message: string; authRejected?: boolean };
      return { type: "userError", requestId: raw.requestId, message: raw.message, authRejected: raw.authRejected ?? false };
    }
    case "taskList": {
      const raw = json as { requestId: number; tasks: Array<Omit<TaskInfo, "running"> & { running?: boolean }>; runsHere: boolean };
      return { type: "taskList", requestId: raw.requestId, tasks: raw.tasks.map((t) => ({ ...t, running: t.running ?? false })), runsHere: raw.runsHere };
    }
    case "nodeList": {
      const raw = json as { requestId: number; nodes: Array<Omit<NodeInfo, "agents" | "requireApproval"> & { agents?: string[]; requireApproval?: boolean }> };
      return { type: "nodeList", requestId: raw.requestId, nodes: raw.nodes.map((n) => ({ ...n, agents: n.agents ?? [], requireApproval: n.requireApproval ?? false })) };
    }
    case "nodeError": {
      const raw = json as { requestId: number; message: string; authRejected?: boolean };
      return { type: "nodeError", requestId: raw.requestId, message: raw.message, authRejected: raw.authRejected ?? false };
    }
    case "taskError": {
      const raw = json as { requestId: number; message: string; authRejected?: boolean };
      return { type: "taskError", requestId: raw.requestId, message: raw.message, authRejected: raw.authRejected ?? false };
    }
    case "deviceError": {
      const raw = json as { requestId: number; message: string; authRejected?: boolean };
      return { type: "deviceError", requestId: raw.requestId, message: raw.message, authRejected: raw.authRejected ?? false };
    }
    case "apiKeyError": {
      const raw = json as { requestId: number; message: string; authRejected?: boolean };
      return { type: "apiKeyError", requestId: raw.requestId, message: raw.message, authRejected: raw.authRejected ?? false };
    }
    case "syncError": {
      const raw = json as { requestId: number; message: string; authRejected?: boolean };
      return { type: "syncError", requestId: raw.requestId, message: raw.message, authRejected: raw.authRejected ?? false };
    }
    case "settingsError": {
      const raw = json as { requestId: number; message: string; conflict?: boolean; authRejected?: boolean };
      return { type: "settingsError", requestId: raw.requestId, message: raw.message, conflict: raw.conflict ?? false, authRejected: raw.authRejected ?? false };
    }
    case "vaultError": {
      const raw = json as { requestId: number; message: string; conflict?: boolean };
      return { type: "vaultError", requestId: raw.requestId, message: raw.message, conflict: raw.conflict ?? false };
    }
    case "history": {
      const raw = json as { requestId: number; messages: Array<Omit<HistoryMessage, "attachments"> & { attachments?: Attachment[] }> };
      return {
        type: "history",
        requestId: raw.requestId,
        messages: raw.messages.map((m) => ({ ...m, attachments: m.attachments ?? [] })),
      };
    }
    case "toolCallRequest": {
      const raw = json as { callId: number; tool: string; arguments: unknown };
      return { type: "toolCallRequest", callId: raw.callId, tool: raw.tool, arguments: raw.arguments };
    }
    default:
      throw new Error(`unknown or unsupported ServerMessage type: ${String(json.type)}`);
  }
}
