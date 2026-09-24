/**
 * TypeScript port of `mobile/lib/services/server_connection.dart`'s `ServerConnection` — same
 * handshake/heartbeat state machine, ported 1:1 where the platform allows. Lives in the
 * background service worker (never the popup, which MV3 destroys on close) — see
 * `background/index.ts` for why, and for the reasoning behind the 20s heartbeat below (Chrome
 * 116+ resets the service worker's idle-shutdown timer on WebSocket message activity).
 *
 * Tool-call handling (Fase 8.3-8.6) also ports `_handleToolCallRequest` from the Dart client.
 * Deliberately still out of scope, same cut line `server_connection.dart` drew for Fase 7.2: no
 * automatic reconnect on a dropped/failed connection.
 */

import { encode, decode, type ClientMessage, type ServerMessage, type SkillDto, type ToolSpec } from "../protocol/messages";

/** A local tool this client can run when the server asks (Fase 8.3-8.6) — `args` is whatever
 * JSON value the model passed as the tool call's arguments. Return the JSON-encodable result, or
 * throw (any exception) to send a `toolCallError` back instead. */
export type ToolHandler = (args: unknown) => Promise<unknown>;

export type ConnectionStatus =
  | { kind: "disconnected"; reason?: string }
  | { kind: "connecting" }
  | { kind: "connected"; serverName: string }
  | { kind: "failure"; message: string };

export class HandshakeError extends Error {}

/**
 * One line of the conversation transcript kept in `background/index.ts`. `ServerConnection`
 * itself only ever produces `"assistant"`/`"error"` entries (from `ChatResponse`/`ChatError`) —
 * `"user"` is a value `background/index.ts` adds directly when it sends a `Chat` message, so the
 * transcript stays complete (what was asked, not just what came back) even if the popup closes
 * and reopens mid-conversation.
 */
export interface ChatEntry {
  role: "user" | "assistant" | "error";
  content: string;
}

type StatusListener = (status: ConnectionStatus) => void;
type ChatListener = (entry: ChatEntry) => void;

export interface ConnectOptions {
  host: string;
  port: number;
  /** wss:// instead of ws:// (P36). */
  secure?: boolean;
  deviceId: string;
  deviceName: string;
  authKey: string;
  deviceToken?: string;
  handshakeTimeoutMs?: number;
  toolSpecs?: ToolSpec[];
  toolHandlers?: Record<string, ToolHandler>;
}

const DEFAULT_HANDSHAKE_TIMEOUT_MS = 10_000;
const HEARTBEAT_INTERVAL_MS = 20_000;
const SKILL_REQUEST_TIMEOUT_MS = 10_000;

interface PendingSkillRequest {
  resolve: (value: SkillDto[] | undefined) => void;
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
  /** P36 — set when the hub turns this connection away mid-session (the device was revoked), so
   * the close that follows reports why instead of "closed unexpectedly". */
  private rejectedReason: string | null = null;
  private nextRequestId = 0;
  private readonly pendingSkillRequests = new Map<number, PendingSkillRequest>();
  private currentStatus: ConnectionStatus;
  private readonly toolHandlers: Record<string, ToolHandler>;

  private constructor(
    socket: WebSocket,
    public readonly serverName: string,
    /** The device token the hub issued in this connection's `helloAck` (P36), if it issued one —
     * the caller must persist it and pass it as `deviceToken` from then on. */
    public readonly issuedDeviceToken: string | undefined,
    toolHandlers: Record<string, ToolHandler>,
  ) {
    this.socket = socket;
    this.toolHandlers = toolHandlers;
    this.currentStatus = { kind: "connected", serverName };
    this.startHeartbeat();
    socket.addEventListener("message", (event) => this.onMessage(decode(event.data as string)));
    socket.addEventListener("error", () => {
      this.stopHeartbeat();
      this.setStatus({ kind: "failure", message: "WebSocket error" });
    });
    socket.addEventListener("close", () => {
      this.stopHeartbeat();
      this.failPendingSkillRequests(new Error("connection closed"));
      this.setStatus(
        this.goodbyeSent
          ? { kind: "disconnected" }
          : { kind: "failure", message: this.rejectedReason !== null ? `authentication rejected: ${this.rejectedReason}` : "Connection closed unexpectedly" },
      );
    });
  }

  get status(): ConnectionStatus {
    return this.currentStatus;
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
    const scheme = options.secure ? "wss" : "ws";
    const socket = new WebSocket(`${scheme}://${options.host}:${options.port}`);
    return ServerConnection.handshake(socket, options);
  }

  private static handshake(socket: WebSocket, options: ConnectOptions): Promise<ServerConnection> {
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
          reject(new HandshakeError(`No response to Hello within ${(options.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS) / 1000}s`));
        });
      }, options.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS);

      const onFirstMessage = (event: MessageEvent) => {
        let reply: ServerMessage;
        try {
          reply = decode(event.data as string);
        } catch (err) {
          finish(() => reject(err instanceof Error ? err : new HandshakeError(String(err))));
          return;
        }
        finish(() => {
          switch (reply.type) {
            case "helloAck":
              resolve(new ServerConnection(socket, reply.serverName, reply.deviceToken, options.toolHandlers ?? {}));
              break;
            case "authError":
              socket.close();
              reject(new HandshakeError(`authentication rejected: ${reply.reason}`));
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
            tools: options.toolSpecs ?? [],
          }),
        );
      });
      socket.addEventListener("message", onFirstMessage);
      socket.addEventListener("error", () => {
        finish(() => reject(new HandshakeError("connection error")));
      });
      socket.addEventListener("close", () => {
        finish(() => reject(new HandshakeError("server closed the connection before replying to Hello")));
      });
    });
  }

  private onMessage(message: ServerMessage): void {
    switch (message.type) {
      case "pong":
        if (message.nonce === this.pendingPingNonce) {
          this.pendingPingNonce = null;
          if (this.currentStatus.kind === "connected") {
            this.setStatus({ kind: "connected", serverName: this.currentStatus.serverName });
          }
        }
        // Nonce mismatch or an unsolicited pong: nothing here depends on strict correlation
        // beyond dead-connection detection, so ignore it — same posture as the Dart client.
        break;
      case "goodbye":
        // The server never actually sends this today (mirrors the Dart client's own comment) —
        // handled for completeness/forward-compatibility.
        this.stopHeartbeat();
        this.setStatus({ kind: "disconnected" });
        break;
      case "chatResponse":
        for (const listener of this.chatListeners) listener({ role: "assistant", content: message.content });
        break;
      case "chatError":
        for (const listener of this.chatListeners) listener({ role: "error", content: message.message });
        break;
      case "toolCallRequest":
        // Fire-and-forget: each call runs independently, so a slow one (e.g. reading a large
        // page) never blocks this connection's heartbeat/chat handling in the meantime — same
        // reasoning as the Dart client's `unawaited(_handleToolCallRequest(...))`.
        void this.handleToolCallRequest(message.callId, message.tool, message.arguments);
        break;
      case "skillList":
        this.settleSkillRequest(message.requestId, (pending) => pending.resolve(message.skills));
        break;
      case "skillOk":
        this.settleSkillRequest(message.requestId, (pending) => pending.resolve(undefined));
        break;
      case "skillError":
        this.settleSkillRequest(message.requestId, (pending) => pending.reject(new Error(message.message)));
        break;
      case "authError":
        this.rejectedReason = message.reason;
        break;
      case "helloAck":
        // Only ever valid as the first frame, already consumed by `handshake`.
        break;
    }
  }

  private async handleToolCallRequest(callId: number, tool: string, args: unknown): Promise<void> {
    const handler = this.toolHandlers[tool];
    if (!handler) {
      this.socket.send(encode({ type: "toolCallError", callId, message: `no local handler registered for tool '${tool}'` }));
      return;
    }
    try {
      const result = await handler(args);
      this.socket.send(encode({ type: "toolCallResult", callId, result }));
    } catch (err) {
      this.socket.send(encode({ type: "toolCallError", callId, message: err instanceof Error ? err.message : String(err) }));
    }
  }

  /** Sends one chat turn. The reply arrives asynchronously via `onChatMessage`. */
  sendChat(message: string): void {
    this.socket.send(encode({ type: "chat", message }));
  }

  /** Skills management (P72) — each call is one request/reply pair correlated by `requestId`
   * (the same idea as a ping's nonce, but several can be in flight, so it's a map). */
  listSkills(): Promise<SkillDto[]> {
    return this.skillRequest((requestId) => ({ type: "listSkills", requestId })).then((skills) => skills ?? []);
  }

  async saveSkill(skill: SkillDto, overwrite: boolean): Promise<void> {
    await this.skillRequest((requestId) => ({ type: "saveSkill", requestId, skill, overwrite }));
  }

  async deleteSkill(name: string): Promise<void> {
    await this.skillRequest((requestId) => ({ type: "deleteSkill", requestId, name }));
  }

  private skillRequest(build: (requestId: number) => ClientMessage): Promise<SkillDto[] | undefined> {
    return new Promise((resolve, reject) => {
      const requestId = this.nextRequestId++;
      const timeoutId = setTimeout(() => {
        this.pendingSkillRequests.delete(requestId);
        reject(new Error("no response from the server"));
      }, SKILL_REQUEST_TIMEOUT_MS);
      this.pendingSkillRequests.set(requestId, { resolve, reject, timeoutId });
      try {
        this.socket.send(encode(build(requestId)));
      } catch (err) {
        this.settleSkillRequest(requestId, (pending) => pending.reject(err instanceof Error ? err : new Error(String(err))));
      }
    });
  }

  private settleSkillRequest(requestId: number, settle: (pending: PendingSkillRequest) => void): void {
    const pending = this.pendingSkillRequests.get(requestId);
    if (!pending) return; // timed out already, or an unsolicited reply
    this.pendingSkillRequests.delete(requestId);
    clearTimeout(pending.timeoutId);
    settle(pending);
  }

  private failPendingSkillRequests(error: Error): void {
    for (const requestId of [...this.pendingSkillRequests.keys()]) {
      this.settleSkillRequest(requestId, (pending) => pending.reject(error));
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
      // The previous ping never got a pong within one full interval — most likely a dead
      // connection. No auto-reconnect in this slice (see file doc) — just surface it.
      this.stopHeartbeat();
      this.setStatus({ kind: "failure", message: "Heartbeat timed out — connection may be dead" });
      return;
    }
    const nonce = this.nextNonce++;
    this.pendingPingNonce = nonce;
    this.socket.send(encode({ type: "ping", nonce }));
  }

  /**
   * Ends the connection cleanly. The server doesn't acknowledge `Goodbye` — it just stops
   * reading and drops the connection — so no reply is awaited; the `close` handler above turns
   * the resulting socket close into a clean `disconnected` status because `goodbyeSent` is set.
   */
  goodbye(reason?: string): void {
    this.goodbyeSent = true;
    this.stopHeartbeat();
    this.socket.send(encode({ type: "goodbye", reason: reason ?? null }));
    this.socket.close();
  }

  private setStatus(status: ConnectionStatus): void {
    this.currentStatus = status;
    for (const listener of this.statusListeners) listener(status);
  }
}
