/** Only the roles ever shown to the human user; warden-core's Role also has
 * System and Tool, but those are internal orchestration detail and never
 * render in the chat UI. */
export type ChatRole = "user" | "assistant";

export interface Usage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

/** An inline image attached to a user message (P28) — base64 with no `data:...;base64,`
 * prefix, mirrors `warden_core::model::Attachment` field for field. */
export interface Attachment {
  mimeType: string;
  data: string;
}

export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  createdAt: number;
  usage?: Usage;
  attachments?: Attachment[];
  /** Paths of files actually written to disk this turn (P64) — `generate_document`'s own
   * result, or oversized MCP media spilled to disk. Feeds the "Open" button in `MessageBubble`. */
  generatedFiles?: string[];
  /** The turn's provider was down and a reserve answered (P79). Shown above the answer, not
   * saved with the conversation. */
  fallbacks?: ProviderFallback[];
}

/** Mirrors `warden_bootstrap::ComboConfig` (P90): provider ids, in order. Its id shares one
 * namespace with the providers'. */
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

export interface Conversation {
  id: string;
  title: string;
  messages: ChatMessage[];
  createdAt: number;
  updatedAt: number;
  /** The agent/model last selected for this conversation (closes P3) — restores the same choice
   * when reopening it. Absent means "no override": no persona, and whatever `activeProvider`
   * currently resolves to. */
  agentId?: string;
  providerId?: string;
  /** The project (P103) this conversation was started in — fixed for its life. Absent: no project, or one that
   * was removed since (the list then shows it with the others). */
  projectId?: string;
  /** The folder of this computer (P102) the conversation works in — picked before its first message and fixed for
   * its life. Absent: none, or the conversation is in a project (which has its own). */
  workdir?: string;
}

/** One project (P103) — mirrors `ProjectPayload` in `src-tauri/src/projects_cmds.rs`. Its files are notes of the
 * vault under `projects/<id>/`. */
export interface ProjectEntry {
  /** The folder name: letters, digits, `-` and `_`. Can't change after saving: conversations point at it. */
  id: string;
  name: string;
  description: string;
  /** What the model is told in every conversation of the project. */
  instructions: string;
  /** A code project's working folder on the hub's machine (P103 b): gives its conversations a shell that asks first. */
  workdir?: string;
  /** Its conversations are driven by a code engine (the opencode) in `workdir`. Needs one. */
  code?: boolean;
}

/** Mirrors `warden_bootstrap::Provider`. `openaiCompatible` covers any other server that speaks
 * the OpenAI chat-completions wire format — Ollama (local, no real key needed), OpenRouter,
 * Groq, DeepSeek, etc. — via a configurable `baseUrl` instead of one dedicated kind per company. */
export type ProviderKind = "gemini" | "openai" | "anthropic" | "openai_compatible" | "node";

/** One entry of the provider registry (Sessão 35) — the user can add/edit/delete any number of
 * these in Settings, each independently selectable as the active one. Empty string means
 * "not set" for every field, including `apiKey` (shown in the UI masked, with a reveal toggle,
 * and directly editable — this app's own explicit choice, same as before the registry existed). */
export interface ProviderEntry {
  /** User-chosen, unique among the list — how `activeProvider` refers to one. */
  id: string;
  kind: ProviderKind;
  apiKey: string;
  /** Only meaningful (and required) for `kind === "openai_compatible"`. */
  baseUrl: string;
  /** Falls back to `defaultModels[kind]` when empty — no such default for `openai_compatible`. For
   * `node`, the id of the provider on that node. */
  model: string;
  /** Kind "node" only (P93): the node's device id. */
  node?: string;
}

/** One external MCP server (Phase 5.2/P25) to connect to on startup — mirrors
 * `warden_bootstrap::McpServerConfig` field for field (no camelCase remapping needed, every
 * field is already a single word). It's an untagged union on the Rust side, discriminated purely
 * by which fields are present (`command` vs `url`) — mirrored here the same way rather than with
 * a synthetic `transport` tag, since that's the actual wire shape. */
export interface McpServerStdio {
  /** Only used for display/error messages — not sent to the server. */
  name: string;
  /** Same shape any MCP client config uses (e.g. Claude Desktop's `mcpServers`). */
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface McpServerHttp {
  /** Only used for display/error messages — not sent to the server. */
  name: string;
  url: string;
  /** Sent on every request — typically just `{ Authorization: "Bearer <token>" }` for a server
   * that authenticates that way. Ignored when `oauth` is true. */
  headers: Record<string, string>;
  /** When true, connect via the OAuth flow (PENDING.md P26) instead of `headers` — discovery,
   * Dynamic Client Registration, browser consent, token refresh. Mutually exclusive with
   * `headers`. Driven from Settings' "Connect"/"Disconnect" buttons, not typed by hand. */
  oauth: boolean;
}

export type McpServer = McpServerStdio | McpServerHttp;

export function isMcpServerHttp(server: McpServer): server is McpServerHttp {
  return "url" in server;
}

/** One named agent (closes P3) — a persona a conversation can pick, alongside its model. Same
 * "flat list, `id` doubles as display name" shape as `ProviderEntry`. */
export interface AgentEntry {
  id: string;
  /** Free text, sent verbatim as a system-prompt message — no structure imposed on it. */
  persona: string;
  /** This agent's default model, referencing a `ProviderEntry.id`. Empty string means "no
   * default" — picking this agent just pre-fills the model selector with this when set, doesn't
   * enforce it afterward. */
  providerId: string;
  /** Opt-in (P46/P60) — when true, a conversation using this agent gets the `delegate_to_agent`
   * tool, letting it address any other configured agent by id. Off by default: this is the only
   * UI surface that can turn it on (previously hand-edit of `config.toml` only). */
  canDelegateToAgents: boolean;
  /** Opt-in (P46) — when true, this agent gets the `manage_agents` tool: it can list, create and edit
   * *other* agents, each change shown to you for approval first. It can never switch this flag, or
   * `canDelegateToAgents`, on for any agent — only this checkbox does. */
  canManageAgents: boolean;
  /** Opt-in (P46, "funcionários" mode) — when true, this agent gets the `message_agent` tool: it can
   * leave a message for another agent, which answers it in a conversation you see in the sidebar. */
  canMessageAgents: boolean;
  /** P92 — the `manage_tasks` tool; only a person turns it on. */
  canManageTasks: boolean;
  /** Tool isolation (P46) — the only tools this agent may use, by name; `null` = every tool. The
   * `delegate_to_agent`/`manage_agents` tools follow the two checkboxes above, never this list. */
  allowedTools: string[] | null;
  /** P122 — how much the agent may do without asking: 1 only answers, 2 suggests, 3 asks before every change, 4 acts alone. */
  autonomy: number;
  /** P122 — the kinds of action (ids, see `lib/approvalCategories.ts`) that need your yes even at autonomy 4. */
  approvalRequired: string[];
  /** P120 — the agent's role in the organization (free text) and the id of the agent it reports to; only shown for now. */
  role?: string | null;
  reportsTo?: string | null;
  /** P84 — the people this agent is shared with, by username, or ["*"] for everyone. */
  sharedWith?: string[];
}

/** Mirrors `ssh_cmds::SshHostPayload` (P47) — an SSH server the AI can run commands on through the
 * `ssh_exec` tool. Only the *path* of a private key is stored, never the key. */
export interface SshHostEntry {
  /** Unique; the only handle the model uses to pick this server. */
  id: string;
  host: string;
  user: string;
  port: number;
  /** Path to a private key — empty string leaves it to ssh-agent and `~/.ssh/config`. */
  identityFile: string;
  /** Master switch: off keeps the host registered but invisible to the model. */
  enabled: boolean;
  /** Agent ids allowed to use it. Empty = every agent and every channel without an agent. */
  agents: string[];
  /** Ask before every command or file transfer on this server (only the desktop and the interactive
   * CLI can ask — any other channel is refused instead). */
  requireApproval: boolean;
}

/** Payload of the `approval-request` event (`approval::ApprovalPayload`) — something the AI wants to
 * do that needs your "yes": an SSH action on a server that requires approval, or creating/changing an
 * agent. Answered through `resolve_approval`. */
/** How much a code conversation asks before the engine acts (P103 b): everything / files changed freely / nothing / no
 * change at all, only a plan. Changeable at any moment, a running task included. */
export type CodeMode = "manual" | "acceptEdits" | "acceptAll" | "plan";

/** What Shift+Tab goes to next: Manual → Accept edits → Plan → Manual. "Accept all" is only ever picked on purpose. */
export function nextCodeMode(mode: CodeMode): CodeMode {
  if (mode === "manual") return "acceptEdits";
  if (mode === "acceptEdits") return "plan";
  return "manual";
}

export interface ApprovalRequest {
  id: number;
  /** The SSH server id or the agent id the action is about. */
  target: string;
  /** `exec` | `upload` | `download` | `create_agent` | `update_agent` | `delete_agent` | `extend_limit` (a spending limit paused the turn, P4). */
  action: string;
  detail: string;
  /** What "Always allow" would cover (e.g. `git status *`), when this ask can be answered that way (P103 b). */
  always?: string | null;
  /** P122 — the kind of action this agent has to get approved (an id of `lib/approvalCategories.ts`), when the ask comes from that rule. */
  category?: string | null;
}

/** Mirrors `warden_bootstrap::GitSyncConfig` (P63/P71 v2) — a self-hosted/remote git repo as an
 * alternative sync transport to Arweave (what the Sync screen's Git section, and the auto-sync
 * loop, use). All-or-nothing on save — see `save_settings`'s validation. */
export interface GitSyncConfig {
  remoteUrl: string;
  token: string;
}

/** What `git_sync_push` returns — `null` when nothing local needed sending. */
export interface GitPushResult {
  commitSha: string;
  filesChanged: number;
  configChanged: boolean;
}

export interface GitPullResult {
  commitsApplied: number;
  filesWritten: number;
  filesDeleted: number;
  /** Skipped because they matched this device's own `.syncignore` (P75) — never written, never deleted. */
  filesIgnored: number;
  configUpdated: boolean;
  warnings: string[];
}

/** One breakdown bucket of a `UsageSummary` — `key` is the `agent_id`/`provider_id` (an
 * `AgentEntry.id`/`ProviderEntry.id`, which already doubles as its display name) `null` means "no
 * override" for that conversation, mirrors `warden_bootstrap::usage::UsageByKey`. */
export interface UsageByKey {
  key: string | null;
  messageCount: number;
  usage: Usage;
}

/** Mirrors `warden_bootstrap::usage::UsageSummary` — the "Usage" nav view's data, aggregated
 * on demand from every saved conversation (see `usage_summary` IPC command). Token counts only,
 * no dollar cost: there's no per-model price table in the project yet. */
export interface UsageSummary {
  conversationCount: number;
  messageCount: number;
  total: Usage;
  byAgent: UsageByKey[];
  byProvider: UsageByKey[];
}

/** Mirrors `warden_server_protocol::protocol::LimitStatusDto` — the same shape the web UI gets. */
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

/** Mirrors `SpendBucketDto` (P10): one model, channel, provider, agent or person in the ledger. */
export interface SpendBucket {
  key: string;
  calls: number;
  tokens: number;
  costUsd: number;
  unpricedCalls: number;
}

/** Mirrors `RecentSpendDto`: what the ledger still holds, which only reaches back `windowHours` (the longest
 * limit). An empty `key` is a call with no provider, agent or person on record. */
export interface RecentSpend {
  windowHours: number;
  byModel: SpendBucket[];
  byChannel: SpendBucket[];
  byProvider: SpendBucket[];
  byAgent: SpendBucket[];
  byPerson: SpendBucket[];
}

/** The Usage screen's limits and recent spending: `spend_status` here, the hub's `UsageReport` there. */
export interface SpendStatus {
  limitsEnabled: boolean;
  limits: LimitStatus[];
  recent: RecentSpend | null;
  ledgerError: string | null;
}

/** Mirrors `warden_server::PairedDevice` (via `workspace_cmds::PairedDeviceInfo`) — one row of the
 * "Workspace" nav view's device list (Fase 9.6). `firstSeenMs`/`lastSeenMs` are epoch
 * milliseconds. Assumes the desktop app runs on the same machine as the `warden-server` hub whose
 * `devices.json` this reads — see `workspace_cmds.rs`'s module docs for why. */
export interface PairedDevice {
  deviceId: string;
  deviceName: string;
  status: "pending" | "approved" | "revoked";
  firstSeenMs: number;
  lastSeenMs: number;
}

/** Mirrors `warden_bootstrap::HubPairingConfig` (via `workspace_cmds::HubPairingConfigPayload`,
 * Fase 9.7) — what the Workspace screen's "Pareamento por QR" section saves and embeds in the QR
 * a new client scans, so it doesn't have to type `serverUrl`/`authKey` by hand. */
export interface HubPairingConfig {
  serverUrl: string;
  authKey: string;
}

/** Mirrors `warden_server_protocol::discovery::DiscoveredHub` (via
 * `workspace_cmds::DiscoveredHubPayload`, Fase 9.1 redefined) — one hub found by a LAN discovery
 * sweep, listed so the operator can pick it instead of typing `serverUrl` by hand. */
export interface DiscoveredHub {
  host: string;
  port: number;
  serverName: string;
  /** Set when the hub only accepts wss:// (P36) — the URL to pair with instead of ws://host:port. */
  secureUrl: string | null;
}

/** Mirrors `TaskDto` (P92): one scheduled task. Exactly one of `every`, `cron` and `once`. */
export interface Task {
  id: string;
  agentId?: string;
  prompt: string;
  every?: string;
  cron?: string;
  once?: string;
  timezone?: string;
  enabled: boolean;
}

/** Where a delegated task is (P123). */
export type AgentTaskState = "pending" | "running" | "waiting" | "done" | "failed" | "cancelled";

/** Mirrors `AgentTaskDto` (P123): a task an agent delegated in the background. `group` is shared by the tasks one turn started. */
export interface AgentTask {
  id: string;
  group: string;
  /** The agent that delegated. */
  owner?: string | null;
  /** The agent that does the work, or the name given to a temporary helper. */
  assignee: string;
  /** The task this one is a subtask of, when its agent started it from inside another task. */
  parentId?: string | null;
  objective: string;
  /** The provider or combo chosen for this task. */
  model?: string | null;
  channel: string;
  state: AgentTaskState;
  result?: string | null;
  error?: string | null;
  promptTokens?: number | null;
  completionTokens?: number | null;
  totalTokens?: number | null;
  createdAtMs: number;
  startedAtMs?: number | null;
  finishedAtMs?: number | null;
}

/** Mirrors `TaskInfoDto`: a task and where it stands on the machine that runs it. */
export interface TaskInfo extends Task {
  nextRunAtMs?: number;
  lastRunAtMs?: number;
  lastFinishedAtMs?: number;
  lastError?: string;
  running: boolean;
  scheduleError?: string;
}

/** Mirrors `task_cmds::TaskListPayload`; on a hub, `runHere` is its `runsHere` and `hubRunning` is always true. */
export interface TaskList {
  tasks: TaskInfo[];
  runHere: boolean;
  hubRunning: boolean;
}

/** One message of a task's or a webhook's conversation, for the read-only history. */
export interface RunMessage {
  role: "user" | "assistant";
  content: string;
  createdAt: number;
}

/** How a webhook's caller proves itself: a bearer token, or an HMAC signature of the body (GitHub, Stripe style). */
export type WebhookAuth = "token" | "hmac";

/** Mirrors `WebhookDto` (P105): one incoming webhook, as `[[webhooks]]` keeps it. */
export interface Webhook {
  id: string;
  agentId?: string;
  prompt: string;
  enabled: boolean;
  auth: WebhookAuth;
}

/** Mirrors `WebhookInfoDto`: a webhook and what the machine that serves it knows about its credential. */
export interface WebhookInfo extends Webhook {
  /** What it has: absent means no credential, so it takes no calls. */
  credential?: WebhookAuth;
  shown?: string;
  createdAtMs?: number;
  lastUsedAtMs?: number;
  conversation: string;
}

/** Mirrors `webhook_cmds::WebhookListPayload`; on a hub, `hubRunning` is its `servesHere` and `hubUrl` its address. */
export interface WebhookList {
  webhooks: WebhookInfo[];
  hubRunning: boolean;
  hubUrl?: string;
}

/** Mirrors `webhook_cmds::WebhookCreatedPayload`: the credential is shown once, here. */
export interface WebhookCreated {
  id: string;
  credential: string;
  kind: WebhookAuth;
  list: WebhookList;
}

/** Mirrors `hub_cmds::HubPayload` (P102) — a hub this computer is a client of: a name and the address of the web
 * interface it serves, and whether its window is open now. */
export interface SavedHub {
  id: string;
  name: string;
  url: string;
  open: boolean;
}

/** Mirrors `warden_bootstrap::EmbeddedServerConfig` (via
 * `server_cmds::EmbeddedServerConfigPayload`, Fase 9.1 follow-up "virar o hub desta rede") — the
 * persisted port/auth key/name for the desktop's own embedded `warden-server`. `enabled` isn't
 * part of this payload on purpose (see `server_cmds.rs`'s module docs) — that lives only in
 * `EmbeddedServerStatus.running`, driven by the start/stop commands, not this form. */
export interface EmbeddedServerConfig {
  port: number;
  /** `serve --listen`'s host — null is 0.0.0.0 (every interface). */
  listenHost: string | null;
  authKey: string;
  serverName: string | null;
  /** P36 — serve only wss://, with this machine's Tailscale certificate. */
  tailscaleCert: boolean;
  /** Or with a certificate of the user's own (`serve --tls-cert`/`--tls-key`/`--tls-host`). */
  tlsCert: string | null;
  tlsKey: string | null;
  tlsHost: string | null;
  /** false is `serve --no-web-ui`. */
  webUi: boolean;
}

/** Mirrors `server_cmds::EmbeddedServerStatusPayload` — whether the embedded server is currently
 * running, and if so, where. */
export interface EmbeddedServerStatus {
  running: boolean;
  boundAddr: string | null;
  serverName: string | null;
  /** Running TLS-only (Tailscale or an own certificate). */
  secure: boolean;
  /** The wss:// URL clients must use — set when running with TLS and a known host name. */
  secureUrl: string | null;
  /** Where to open the hub's web interface (P78) — null when it's off, this build has none
   * compiled in, or TLS runs without a host name for the link. */
  webUrl: string | null;
}

/** Mirrors `warden_sync::SyncStatus` (via `sync_cmds::SyncStatusPayload`) — the "Sync" nav view's
 * status card. `null` fields mean "not applicable yet" (e.g. `deviceId` before `sync_init`/
 * pairing, `ownerAddress`/`lastTxId`/`lastSyncedAtMs` before the first push or pull). */
export interface SyncStatus {
  paired: boolean;
  deviceId: string | null;
  ownerAddress: string | null;
  lastTxId: string | null;
  lastSyncedAtMs: number | null;
  pendingVaultChanges: number;
  pendingConfigChanged: boolean;
  /** How many patterns are active in `.syncignore` (P75) at the vault root — 0 when it doesn't exist. */
  syncignorePatternCount: number;
}

/** What `sync_push_begin` returns — the QR to show before blocking on `sync_push_await`. */
export interface SyncPushBegin {
  qrSvg: string;
  filesChanged: number;
  configChanged: boolean;
}

export interface SyncPushResult {
  txId: string;
  filesChanged: number;
  configChanged: boolean;
}

export interface SyncPullResult {
  txId: string | null;
  filesWritten: number;
  filesDeleted: number;
  /** Skipped because they matched this device's own `.syncignore` (P75) — never written, never deleted. */
  filesIgnored: number;
  configUpdated: boolean;
  warnings: string[];
}

/** What `get_settings` returns, and also what the settings form holds — the shapes are
 * identical so the fetched snapshot can be used directly as initial form state. */
/** One skill (P16) — mirrors `SkillPayload` in `src-tauri/src/skills_cmds.rs`. */
export interface SkillEntry {
  /** Slug (lowercase letters, digits, hyphens); doubles as the filename under `skills/`. */
  name: string;
  description: string;
  body: string;
  /** Agent ids the skill is restricted to (P72 c); empty = every agent sees it. */
  agents: string[];
  /** P104 — a suggestion the assistant made after a conversation, not yet accepted. Saving keeps it
   * pending unless the editor's "Accept" box is ticked. */
  proposed?: boolean;
  /** The conversation that suggested it, and when (ms since epoch). Only set while `proposed`. */
  source?: string;
  proposedAt?: number;
  /** P115: the suggestion is a change to this existing skill; accepting applies it there. */
  revises?: string;
}

/** P84 — `person` is one member of the workspace, on every channel. */
export type LimitScope = "global" | "agent" | "channel" | "user" | "person";

/** Mirrors `spend_cmds::LimitPayload` (P4) — one spending ceiling: at most `maxTokens` and/or
 * `maxCostUsd` inside any sliding `windowHours` stretch. `target` is an empty string for `global`;
 * an agent id, a channel name, or `channel:user` otherwise. `warnAt`/`extendStep` are fractions
 * (0–1) of the ceiling; `null` = the default (0.8 and 0.25). */
export interface LimitEntry {
  id: string;
  scope: LimitScope;
  target: string;
  windowHours: number;
  maxTokens: number | null;
  maxCostUsd: number | null;
  warnAt: number | null;
  extendStep: number | null;
}

/** Mirrors `spend_cmds::PricePayload` — dollars per million tokens for the model with exactly this id. */
export interface PriceEntry {
  model: string;
  inputPerMtok: number;
  outputPerMtok: number;
}

export interface Settings {
  providers: ProviderEntry[];
  /** `id` of the `providers` entry currently in use — empty string means none selected. */
  activeProvider: string;
  /** Named routing combos (P90): picked like a provider (active model, an agent's default, a
   * conversation's model); tries its providers in order when one is down. */
  combos: Combo[];
  vaultPath: string;
  /** Where `generate_document` and oversized MCP media (P64/P66) get written — empty string
   * means "unset", resolving at bootstrap time to a sibling of the vault path. */
  generatedPath: string;
  tavilyKey: string;
  /** OpenAI API key for Whisper transcription (P28 part 2) — dedicated, independent of which
   * provider is active for chat, so voice input works no matter which one is selected. */
  whisperKey: string;
  /** Opt-in gate for the `shell` tool — off by default, since it lets the model run arbitrary
   * commands on this machine with no sandboxing. */
  enableShell: boolean;
  /** Default model per provider kind (`"gemini"`/`"openai"`/`"anthropic"`), shown as the Model
   * field's placeholder — no entry for `openaiCompatible`. */
  defaultModels: Record<string, string>;
  mcpServers: McpServer[];
  agents: AgentEntry[];
  /** SSH servers the AI can run commands on (P47). */
  sshHosts: SshHostEntry[];
  /** Connection details for the git sync backend (P63/P71) — `null` until filled in on the "Sync
   * via Git" section. */
  gitSync: GitSyncConfig | null;
  /** Spending limits (P4). `null` = none written, so the built-in safety net is in force; `[]` =
   * every limit switched off. Different on purpose. */
  limits: LimitEntry[] | null;
  /** The safety net as editable entries, for "Customize" to start from. */
  defaultLimits: LimitEntry[];
  /** `WARDEN_SPEND_LIMITS=off` in the environment beats whatever `limits` says. */
  limitsDisabledByEnv: boolean;
  /** What each model charges — nothing is built in, so a model with no entry has no `$` figure. */
  prices: PriceEntry[];
  /** The config file's version when this was read, sent back on save: the hub's web settings (P78)
   * write the same file, and a save over their change is refused instead of undoing it. */
  version: string;
}
