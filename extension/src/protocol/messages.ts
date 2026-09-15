/**
 * Mirrors `crates/warden-server-protocol/src/protocol.rs` — the subset this extension's Fase
 * 8.1/8.2 slice needs (no tool-call variants yet, since no tool is advertised in `Hello.tools`
 * until Fase 8.3-8.6 exist). Wire shape: internally-tagged JSON with a `type` field, both the tag
 * and every field name camelCase (`#[serde(tag = "type", rename_all = "camelCase",
 * rename_all_fields = "camelCase")]` on the Rust side) — locked by `protocol.rs`'s own
 * round-trip tests, not guessed.
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

export type ClientMessage =
  | { type: "hello"; deviceId: string; deviceName: string; authKey: string; tools: [] }
  | { type: "ping"; nonce: number }
  | { type: "chat"; message: string }
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
  | { type: "goodbye"; reason: string | null };

/**
 * Decodes one `ServerMessage`. Throws on anything this slice doesn't understand — including the
 * tool-call variants `warden-server` may still send if the model happens to try one server-side
 * (never true today since `Hello.tools` is always `[]` here, but explicit is better than a silent
 * `as` cast producing a message shape this client can't actually handle).
 */
export function decode(text: string): ServerMessage {
  const json = JSON.parse(text) as { type?: unknown };
  switch (json.type) {
    case "helloAck":
    case "authError":
    case "pong":
    case "chatError":
    case "goodbye":
      return json as ServerMessage;
    case "chatResponse": {
      const raw = json as { content: string; usage: Usage | null; attachments?: Attachment[] };
      return { type: "chatResponse", content: raw.content, usage: raw.usage, attachments: raw.attachments ?? [] };
    }
    default:
      throw new Error(`unknown or unsupported ServerMessage type: ${String(json.type)}`);
  }
}
