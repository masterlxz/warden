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

/** Mirrors `warden_server_protocol::protocol::SkillDto` (P72). `agents` is the agent restriction
 * (empty = every agent) — this client only displays it; an edit that sends `[]` keeps whatever the
 * server has stored. */
export interface SkillDto {
  name: string;
  description: string;
  body: string;
  agents: string[];
}

/** Mirrors `warden_server_protocol::protocol::ConversationSummary` (P78). */
export interface ConversationSummary {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
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
  limitsEnabled: boolean;
  limits: LimitStatus[];
  recent?: { windowHours: number; byModel: SpendBucket[]; byChannel: SpendBucket[] };
  ledgerError?: string;
}

/** Mirrors `SecretStatusDto` (P78): whether an API key is saved, never the key itself. `hint` is its
 * last four characters, only for a key long enough that they give nothing away. */
export interface SecretStatus {
  set: boolean;
  hint?: string;
}

/** Mirrors `SecretEdit`: what a save does to one secret. `keep` is an untouched field. */
export type SecretEdit = { action: "keep" } | { action: "set"; value: string } | { action: "clear" };

export type ProviderKind = "gemini" | "openai" | "anthropic" | "openai_compatible";

/** Mirrors `ProviderSettingsDto`. Empty strings mean "not set". */
export interface ProviderSettings {
  id: string;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  apiKey: SecretStatus;
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
  allowedTools: string[] | null;
}

export type LimitScope = "global" | "agent" | "channel" | "user";

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
  notes: string[];
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
}

export type ClientMessage =
  /** `authKey` is the hub's pairing key (P36), only needed until this device holds a
   * `deviceToken` from an earlier `helloAck`. */
  | { type: "hello"; deviceId: string; deviceName: string; authKey: string; deviceToken?: string; tools: ToolSpec[] }
  | { type: "ping"; nonce: number }
  /** `conversationId` (P78) picks one of this device's conversations — a new id starts a new one;
   * omitted, the turn goes to the device's default conversation. */
  | { type: "chat"; message: string; conversationId?: string; attachments?: Attachment[] }
  | { type: "toolCallResult"; callId: number; result: unknown }
  | { type: "toolCallError"; callId: number; message: string }
  /** Skills management (P72) — `requestId` is echoed on the matching reply. */
  | { type: "listSkills"; requestId: number }
  | { type: "saveSkill"; requestId: number; skill: SkillDto; overwrite: boolean }
  | { type: "deleteSkill"; requestId: number; name: string }
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
  | { type: "helloAck"; serverName: string; deviceToken?: string }
  | { type: "authError"; reason: string }
  | { type: "pong"; nonce: number }
  /** `conversationId` (P78) — which conversation this answers; `chat` has no `requestId`. */
  | { type: "chatResponse"; content: string; usage: Usage | null; attachments: Attachment[]; conversationId?: string }
  /** `spendLimitId` (P4/P78): the turn stopped on that spending limit — offer `extendLimit`. */
  | { type: "chatError"; message: string; conversationId?: string; spendLimitId?: string }
  | { type: "toolCallRequest"; callId: number; tool: string; arguments: unknown }
  | { type: "skillList"; requestId: number; skills: SkillDto[] }
  | { type: "skillOk"; requestId: number }
  | { type: "skillError"; requestId: number; message: string }
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
      const raw = json as { content: string; usage: Usage | null; attachments?: Attachment[]; conversationId?: string };
      return { type: "chatResponse", content: raw.content, usage: raw.usage, attachments: raw.attachments ?? [], conversationId: raw.conversationId };
    }
    case "skillList": {
      const raw = json as { requestId: number; skills: Array<Omit<SkillDto, "agents"> & { agents?: string[] }> };
      return { type: "skillList", requestId: raw.requestId, skills: raw.skills.map((skill) => ({ ...skill, agents: skill.agents ?? [] })) };
    }
    case "skillOk":
    case "skillError":
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
      return json as ServerMessage;
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
