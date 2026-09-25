/**
 * The web UI's connection to the hub (P78) — adapted from the extension's
 * `extension/src/background/connection.ts` (itself a port of the mobile client): same
 * `hello`/`helloAck` handshake, 20s ping/pong heartbeat, and `requestId`-correlated requests. What
 * differs: it connects to a full URL (the page's own origin, see `hubUrl`), it advertises no local
 * tools (a browser tab has nothing for the model to run), and chat entries carry the reply's
 * attachments so the page can show them. Reconnecting is `App.tsx`'s job, not this class's.
 */

import {
  encode,
  decode,
  type Attachment,
  type ClientMessage,
  type ConversationSummary,
  type HistoryMessage,
  type ServerMessage,
  type SkillDto,
  type VaultSearchHit,
} from "./messages";

export type ConnectionStatus =
  | { kind: "disconnected"; reason?: string }
  | { kind: "connecting" }
  | { kind: "connected"; serverName: string }
  | { kind: "failure"; message: string };

export class HandshakeError extends Error {
  /** Set when the hub itself turned this device away (`authError`), not a network failure. */
  constructor(
    message: string,
    public readonly authRejected = false,
  ) {
    super(message);
  }
}

/** A vault save/delete the hub refused because the note changed since it was opened (P78). */
export class VaultConflictError extends Error {}

/** A note as opened for editing — `version` goes back with the save. */
export interface VaultNote {
  content: string;
  version: string;
}

/** One line of the transcript. `error` entries come from `chatError` or a failed request. */
export interface ChatEntry {
  role: "user" | "assistant" | "error";
  content: string;
  attachments: Attachment[];
}

export function historyToEntries(messages: HistoryMessage[]): ChatEntry[] {
  return messages.map((m) => ({ role: m.role, content: m.content, attachments: m.attachments }));
}

type StatusListener = (status: ConnectionStatus) => void;
/** `conversationId` is the conversation the reply belongs to (P78) — the hub always sends it. */
type ChatListener = (entry: ChatEntry, conversationId: string | undefined) => void;

export interface ConnectOptions {
  url: string;
  deviceId: string;
  deviceName: string;
  /** The hub's pairing key (P36) — only needed until this browser holds a `deviceToken`. */
  authKey: string;
  deviceToken?: string;
  handshakeTimeoutMs?: number;
}

const DEFAULT_HANDSHAKE_TIMEOUT_MS = 10_000;
const HEARTBEAT_INTERVAL_MS = 20_000;
const REQUEST_TIMEOUT_MS = 15_000;
const TRANSCRIBE_TIMEOUT_MS = 60_000;

/** Where the hub is: the page's own origin (the hub serves this page on its WebSocket port), or
 * `VITE_HUB_URL` under `npm run dev`. */
export function hubUrl(): string {
  const override = import.meta.env.VITE_HUB_URL;
  if (override) return override;
  const scheme = window.location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${window.location.host}`;
}

/** One in-flight request/reply pair (skills, history), keyed by `requestId`. */
interface PendingRequest {
  resolve: (reply: ServerMessage) => void;
  reject: (error: Error) => void;
  timeoutId: ReturnType<typeof setTimeout>;
}

export class ServerConnection {
  private readonly socket: WebSocket;
  private readonly statusListeners = new Set<StatusListener>();
  private readonly chatListeners = new Set<ChatListener>();
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private nextNonce = 0;
  private pendingPingNonce: number | null = null;
  private goodbyeSent = false;
  /** Set when the hub turns this connection away mid-session (the device was revoked), so the
   * close that follows reports why instead of "closed unexpectedly". */
  private rejectedReason: string | null = null;
  private nextRequestId = 0;
  private readonly pendingRequests = new Map<number, PendingRequest>();
  private currentStatus: ConnectionStatus;

  private constructor(
    socket: WebSocket,
    public readonly serverName: string,
    /** The device token the hub issued in this connection's `helloAck`, if it issued one — the
     * caller must persist it and pass it as `deviceToken` from then on. */
    public readonly issuedDeviceToken: string | undefined,
  ) {
    this.socket = socket;
    this.currentStatus = { kind: "connected", serverName };
    this.startHeartbeat();
    socket.addEventListener("message", (event) => {
      try {
        this.onMessage(decode(event.data as string));
      } catch (err) {
        console.warn("warden: ignoring a message this page doesn't understand", err);
      }
    });
    socket.addEventListener("close", () => {
      this.stopHeartbeat();
      this.failPendingRequests(new Error("connection closed"));
      this.setStatus(
        this.goodbyeSent
          ? { kind: "disconnected" }
          : { kind: "failure", message: this.rejectedReason !== null ? `authentication rejected: ${this.rejectedReason}` : "a conexão com o hub caiu" },
      );
    });
  }

  get status(): ConnectionStatus {
    return this.currentStatus;
  }

  /** True once the hub revoked this device mid-session — reconnecting with the same token is pointless. */
  get wasRejected(): boolean {
    return this.rejectedReason !== null;
  }

  onStatusChange(listener: StatusListener): () => void {
    this.statusListeners.add(listener);
    return () => this.statusListeners.delete(listener);
  }

  onChatMessage(listener: ChatListener): () => void {
    this.chatListeners.add(listener);
    return () => this.chatListeners.delete(listener);
  }

  static connect(options: ConnectOptions): Promise<ServerConnection> {
    let socket: WebSocket;
    try {
      socket = new WebSocket(options.url);
    } catch (err) {
      return Promise.reject(new HandshakeError(err instanceof Error ? err.message : String(err)));
    }
    return ServerConnection.handshake(socket, options);
  }

  private static handshake(socket: WebSocket, options: ConnectOptions): Promise<ServerConnection> {
    const timeoutMs = options.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS;
    return new Promise((resolve, reject) => {
      let settled = false;

      const finish = (fn: () => void) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeoutId);
        socket.removeEventListener("message", onFirstMessage);
        fn();
      };

      const timeoutId = setTimeout(() => {
        finish(() => {
          socket.close();
          reject(new HandshakeError(`o hub não respondeu em ${timeoutMs / 1000}s`));
        });
      }, timeoutMs);

      const onFirstMessage = (event: MessageEvent) => {
        let reply: ServerMessage;
        try {
          reply = decode(event.data as string);
        } catch (err) {
          finish(() => reject(new HandshakeError(err instanceof Error ? err.message : String(err))));
          return;
        }
        finish(() => {
          switch (reply.type) {
            case "helloAck":
              resolve(new ServerConnection(socket, reply.serverName, reply.deviceToken));
              break;
            case "authError":
              socket.close();
              reject(new HandshakeError(reply.reason, true));
              break;
            default:
              socket.close();
              reject(new HandshakeError(`expected helloAck, got ${reply.type}`));
          }
        });
      };

      socket.addEventListener("open", () => {
        socket.send(
          encode({
            type: "hello",
            deviceId: options.deviceId,
            deviceName: options.deviceName,
            authKey: options.authKey,
            ...(options.deviceToken !== undefined && { deviceToken: options.deviceToken }),
            tools: [],
          }),
        );
      });
      socket.addEventListener("message", onFirstMessage);
      socket.addEventListener("error", () => {
        finish(() => reject(new HandshakeError("não foi possível falar com o hub")));
      });
      socket.addEventListener("close", () => {
        finish(() => reject(new HandshakeError("o hub fechou a conexão antes de responder")));
      });
    });
  }

  private onMessage(message: ServerMessage): void {
    switch (message.type) {
      case "pong":
        if (message.nonce === this.pendingPingNonce) this.pendingPingNonce = null;
        break;
      case "goodbye":
        this.stopHeartbeat();
        this.setStatus({ kind: "disconnected" });
        break;
      case "chatResponse":
        for (const listener of this.chatListeners) {
          listener({ role: "assistant", content: message.content, attachments: message.attachments }, message.conversationId);
        }
        break;
      case "chatError":
        for (const listener of this.chatListeners) listener({ role: "error", content: message.message, attachments: [] }, message.conversationId);
        break;
      case "toolCallRequest":
        // Never advertised any tools, so the hub shouldn't ask — answer anyway so it isn't left waiting.
        this.socket.send(encode({ type: "toolCallError", callId: message.callId, message: "the web UI has no local tools" }));
        break;
      case "skillList":
      case "skillOk":
      case "history":
      case "conversationList":
      case "conversationOk":
      case "transcription":
      case "vaultFileList":
      case "vaultNote":
      case "vaultSaved":
      case "vaultOk":
      case "vaultSearchResults":
        this.settleRequest(message.requestId, (pending) => pending.resolve(message));
        break;
      case "vaultError":
        this.settleRequest(message.requestId, (pending) =>
          pending.reject(message.conflict ? new VaultConflictError(message.message) : new Error(message.message)),
        );
        break;
      case "skillError":
      case "historyError":
      case "conversationError":
      case "transcriptionError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new Error(message.message)));
        break;
      case "authError":
        this.rejectedReason = message.reason;
        break;
      case "helloAck":
      case "discoverAck":
        // `helloAck` is only valid as the first frame (consumed by `handshake`); `discoverAck` never follows a Hello.
        break;
    }
  }

  /** Sends one chat turn to `conversationId` (a new id starts a new conversation), with any
   * images/PDFs. The reply arrives asynchronously via `onChatMessage`, tagged with the same id. */
  sendChat(message: string, conversationId: string, attachments: Attachment[] = []): void {
    this.socket.send(encode({ type: "chat", message, conversationId, ...(attachments.length > 0 && { attachments }) }));
  }

  /** P78 — the hub's transcription of a voice recording. Whisper can take a while on a long clip,
   * hence the longer wait than other requests. */
  async transcribe(audio: Attachment): Promise<string> {
    const reply = await this.request((requestId) => ({ type: "transcribe", requestId, audio }), TRANSCRIBE_TIMEOUT_MS);
    return reply.type === "transcription" ? reply.text : "";
  }

  /** This device's conversations on the hub, newest-updated first. */
  async listConversations(): Promise<ConversationSummary[]> {
    const reply = await this.request((requestId) => ({ type: "listConversations", requestId }));
    return reply.type === "conversationList" ? reply.conversations : [];
  }

  async renameConversation(conversationId: string, title: string): Promise<void> {
    await this.request((requestId) => ({ type: "renameConversation", requestId, conversationId, title }));
  }

  async deleteConversation(conversationId: string): Promise<void> {
    await this.request((requestId) => ({ type: "deleteConversation", requestId, conversationId }));
  }

  async listSkills(): Promise<SkillDto[]> {
    const reply = await this.request((requestId) => ({ type: "listSkills", requestId }));
    return reply.type === "skillList" ? reply.skills : [];
  }

  async saveSkill(skill: SkillDto, overwrite: boolean): Promise<void> {
    await this.request((requestId) => ({ type: "saveSkill", requestId, skill, overwrite }));
  }

  async deleteSkill(name: string): Promise<void> {
    await this.request((requestId) => ({ type: "deleteSkill", requestId, name }));
  }

  /** Every file in the hub's vault a person can browse (not the fixed files, `skills/` or dotfiles). */
  async listVaultFiles(): Promise<string[]> {
    const reply = await this.request((requestId) => ({ type: "listVaultFiles", requestId }));
    return reply.type === "vaultFileList" ? reply.files : [];
  }

  async readVaultNote(path: string): Promise<VaultNote> {
    const reply = await this.request((requestId) => ({ type: "readVaultNote", requestId, path }));
    if (reply.type !== "vaultNote") throw new Error("resposta inesperada do hub");
    return { content: reply.content, version: reply.version };
  }

  /** Saves a note and returns its new version. Without `expectedVersion` it creates the note.
   * Rejects with `VaultConflictError` if the note changed since `expectedVersion`. */
  async saveVaultNote(path: string, content: string, expectedVersion?: string): Promise<string> {
    const reply = await this.request((requestId) => ({
      type: "saveVaultNote",
      requestId,
      path,
      content,
      ...(expectedVersion !== undefined && { expectedVersion }),
    }));
    if (reply.type !== "vaultSaved") throw new Error("resposta inesperada do hub");
    return reply.version;
  }

  async deleteVaultNote(path: string, expectedVersion: string): Promise<void> {
    await this.request((requestId) => ({ type: "deleteVaultNote", requestId, path, expectedVersion }));
  }

  async searchVault(query: string): Promise<VaultSearchHit[]> {
    const reply = await this.request((requestId) => ({ type: "searchVault", requestId, query }));
    return reply.type === "vaultSearchResults" ? reply.hits : [];
  }

  /** One of this device's conversations on the hub, oldest first (the most recent `limit`). A
   * conversation that doesn't exist yet comes back empty. */
  async fetchHistory(conversationId: string, limit: number): Promise<HistoryMessage[]> {
    const reply = await this.request((requestId) => ({ type: "requestHistory", requestId, limit, conversationId }));
    return reply.type === "history" ? reply.messages : [];
  }

  private request(build: (requestId: number) => ClientMessage, timeoutMs = REQUEST_TIMEOUT_MS): Promise<ServerMessage> {
    return new Promise((resolve, reject) => {
      const requestId = this.nextRequestId++;
      const timeoutId = setTimeout(() => {
        this.pendingRequests.delete(requestId);
        reject(new Error("o hub não respondeu"));
      }, timeoutMs);
      this.pendingRequests.set(requestId, { resolve, reject, timeoutId });
      try {
        this.socket.send(encode(build(requestId)));
      } catch (err) {
        this.settleRequest(requestId, (pending) => pending.reject(err instanceof Error ? err : new Error(String(err))));
      }
    });
  }

  private settleRequest(requestId: number, settle: (pending: PendingRequest) => void): void {
    const pending = this.pendingRequests.get(requestId);
    if (!pending) return; // timed out already, or an unsolicited reply
    this.pendingRequests.delete(requestId);
    clearTimeout(pending.timeoutId);
    settle(pending);
  }

  private failPendingRequests(error: Error): void {
    for (const requestId of [...this.pendingRequests.keys()]) {
      this.settleRequest(requestId, (pending) => pending.reject(error));
    }
  }

  private startHeartbeat(): void {
    this.heartbeatTimer = setInterval(() => this.pingNow(), HEARTBEAT_INTERVAL_MS);
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer !== null) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private pingNow(): void {
    if (this.pendingPingNonce !== null) {
      // The previous ping never got a pong within one full interval — treat the connection as
      // dead; closing it fires the `close` handler, and `App.tsx` reconnects from there.
      this.socket.close();
      return;
    }
    const nonce = this.nextNonce++;
    this.pendingPingNonce = nonce;
    this.socket.send(encode({ type: "ping", nonce }));
  }

  /** Ends the connection cleanly (the hub doesn't acknowledge `goodbye`). */
  goodbye(reason?: string): void {
    this.goodbyeSent = true;
    this.stopHeartbeat();
    if (this.socket.readyState === WebSocket.OPEN) this.socket.send(encode({ type: "goodbye", reason: reason ?? null }));
    this.socket.close();
  }

  private setStatus(status: ConnectionStatus): void {
    this.currentStatus = status;
    for (const listener of this.statusListeners) listener(status);
  }
}
