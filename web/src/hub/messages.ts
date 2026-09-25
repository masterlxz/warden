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

export type ClientMessage =
  /** `authKey` is the hub's pairing key (P36), only needed until this device holds a
   * `deviceToken` from an earlier `helloAck`. */
  | { type: "hello"; deviceId: string; deviceName: string; authKey: string; deviceToken?: string; tools: ToolSpec[] }
  | { type: "ping"; nonce: number }
  /** `conversationId` (P78) picks one of this device's conversations — a new id starts a new one;
   * omitted, the turn goes to the device's default conversation. */
  | { type: "chat"; message: string; conversationId?: string }
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
  | { type: "chatError"; message: string; conversationId?: string }
  | { type: "toolCallRequest"; callId: number; tool: string; arguments: unknown }
  | { type: "skillList"; requestId: number; skills: SkillDto[] }
  | { type: "skillOk"; requestId: number }
  | { type: "skillError"; requestId: number; message: string }
  | { type: "history"; requestId: number; messages: HistoryMessage[] }
  | { type: "historyError"; requestId: number; message: string }
  | { type: "conversationList"; requestId: number; conversations: ConversationSummary[] }
  | { type: "conversationOk"; requestId: number }
  | { type: "conversationError"; requestId: number; message: string }
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
      return json as ServerMessage;
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
