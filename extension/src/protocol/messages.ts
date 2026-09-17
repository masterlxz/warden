/**
 * Mirrors `crates/warden-server-protocol/src/protocol.rs` — now including the tool-call variants
 * (Fase 8.3-8.6), since `Hello.tools` is no longer always empty (see `background/tools/index.ts`).
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

export type ClientMessage =
  | { type: "hello"; deviceId: string; deviceName: string; authKey: string; tools: ToolSpec[] }
  | { type: "ping"; nonce: number }
  | { type: "chat"; message: string }
  | { type: "toolCallResult"; callId: number; result: unknown }
  | { type: "toolCallError"; callId: number; message: string }
  /** Fase 9.1 (redefined) — an unauthenticated presence probe, answered by `discoverAck` below.
   * No `authKey`/`deviceId` on purpose: the point is finding a hub before knowing its credential. */
  | { type: "discover" }
  | { type: "goodbye"; reason: string | null };

export function encode(message: ClientMessage): string {
  return JSON.stringify(message);
}

export type ServerMessage =
  | { type: "helloAck"; serverName: string }
  | { type: "authError"; reason: string }
  | { type: "pong"; nonce: number }
  | { type: "chatResponse"; content: string; usage: Usage | null; attachments: Attachment[] }
  | { type: "chatError"; message: string }
  | { type: "toolCallRequest"; callId: number; tool: string; arguments: unknown }
  /** Reply to `ClientMessage.discover` — just enough to let the operator recognize which machine
   * this is, never a secret. */
  | { type: "discoverAck"; serverName: string }
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
      const raw = json as { content: string; usage: Usage | null; attachments?: Attachment[] };
      return { type: "chatResponse", content: raw.content, usage: raw.usage, attachments: raw.attachments ?? [] };
    }
    case "toolCallRequest": {
      const raw = json as { callId: number; tool: string; arguments: unknown };
      return { type: "toolCallRequest", callId: raw.callId, tool: raw.tool, arguments: raw.arguments };
    }
    default:
      throw new Error(`unknown or unsupported ServerMessage type: ${String(json.type)}`);
  }
}
