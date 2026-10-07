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

import {
  encode,
  decode,
  type AgentTask,
  type AgentTaskAction,
  type ApprovalPrompt,
  type ClientMessage,
  type ConversationSummary,
  type DirListing,
  type HistoryMessage,
  type HubAgents,
  type OrgEdit,
  type ServerMessage,
  type SkillDto,
  type ThreadParent,
  type ToolSpec,
} from "../protocol/messages";

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

/** A request the hub refused. `authRejected`: the pairing key was wrong, nothing changed. */
export class HubRequestError extends Error {
  constructor(
    message: string,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

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
  /** P125 — the hub's id for the message, which a thread attaches to. Missing on what has just been sent or answered, until the history is read again. */
  id?: string;
}

type StatusListener = (status: ConnectionStatus) => void;
/** `conversationId` is the conversation the reply belongs to (P78). */
type ChatListener = (entry: ChatEntry, conversationId: string | undefined) => void;
/** P87 — a new approval to show, or one the hub stopped waiting on. */
export type ApprovalEvent = { kind: "prompt"; prompt: ApprovalPrompt } | { kind: "cancelled"; approvalId: number };
type ApprovalListener = (event: ApprovalEvent) => void;

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
const REQUEST_TIMEOUT_MS = 15_000;
/** A change to the settings starts the hub's orchestrator again, which can take a while (MCP servers). */
const SAVE_SETTINGS_TIMEOUT_MS = 120_000;

/** One in-flight request/reply pair (skills P72, history P40), keyed by `requestId`. `resolve`
 * gets the matching success reply; an `*Error` reply rejects instead. */
interface PendingRequest {
  resolve: (reply: ServerMessage) => void;
  reject: (error: Error) => void;
  timeoutId: ReturnType<typeof setTimeout>;
}

export class ServerConnection {
  private readonly socket: WebSocket;
  private readonly statusListeners = new Set<StatusListener>();
  private readonly chatListeners = new Set<ChatListener>();
  private readonly approvalListeners = new Set<ApprovalListener>();
  private readonly changedListeners = new Set<(conversationId: string) => void>();
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private nextNonce = 0;
  private pendingPingNonce: number | null = null;
  private goodbyeSent = false;
  /** P36 — set when the hub turns this connection away mid-session (the device was revoked), so
   * the close that follows reports why instead of "closed unexpectedly". */
  private rejectedReason: string | null = null;
  private nextRequestId = 0;
  private readonly pendingRequests = new Map<number, PendingRequest>();
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
    socket.addEventListener("message", (event) => {
      let message: ServerMessage;
      try {
        message = decode(event.data as string);
      } catch (err) {
        // A message this client doesn't know yet (a newer hub) is skipped, not a broken connection.
        console.warn("warden:", err);
        return;
      }
      this.onMessage(message);
    });
    socket.addEventListener("error", () => {
      this.stopHeartbeat();
      this.setStatus({ kind: "failure", message: "WebSocket error" });
    });
    socket.addEventListener("close", () => {
      this.stopHeartbeat();
      this.failPendingRequests(new Error("connection closed"));
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

  /** P87 — a tool in this device's turn waits on the person, or the hub stopped waiting. */
  onApproval(listener: ApprovalListener): () => void {
    this.approvalListeners.add(listener);
    return () => this.approvalListeners.delete(listener);
  }

  /** P87 — one of this device's conversations changed outside a chat reply. */
  onConversationChanged(listener: (conversationId: string) => void): () => void {
    this.changedListeners.add(listener);
    return () => this.changedListeners.delete(listener);
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
        for (const listener of this.chatListeners) listener({ role: "assistant", content: message.content }, message.conversationId);
        break;
      case "chatError":
        for (const listener of this.chatListeners) listener({ role: "error", content: message.message }, message.conversationId);
        break;
      case "toolCallRequest":
        // Fire-and-forget: each call runs independently, so a slow one (e.g. reading a large
        // page) never blocks this connection's heartbeat/chat handling in the meantime — same
        // reasoning as the Dart client's `unawaited(_handleToolCallRequest(...))`.
        void this.handleToolCallRequest(message.callId, message.tool, message.arguments);
        break;
      case "approvalRequest": {
        const { approvalId, target, action, detail, category } = message;
        for (const listener of this.approvalListeners) listener({ kind: "prompt", prompt: { approvalId, target, action, detail, category } });
        break;
      }
      case "approvalCancelled":
        for (const listener of this.approvalListeners) listener({ kind: "cancelled", approvalId: message.approvalId });
        break;
      case "conversationsChanged":
        for (const listener of this.changedListeners) listener(message.conversationId);
        break;
      case "settings":
      case "settingsSaved":
      case "agentTaskList":
      case "skillList":
      case "skillOk":
      case "history":
      case "conversationList":
      case "conversationOk":
      case "agentChannel":
      case "dirList":
        this.settleRequest(message.requestId, (pending) => pending.resolve(message));
        break;
      case "dirError":
      case "skillError":
      case "historyError":
      case "conversationError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new Error(message.message)));
        break;
      case "settingsError":
      case "taskError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new HubRequestError(message.message, message.authRejected ?? false)));
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

  /** Sends one chat turn to `conversationId` (a new id starts a new conversation, P78). The reply
   * arrives asynchronously via `onChatMessage`, tagged with the same id. */
  sendChat(message: string, conversationId: string, agentId?: string, workdir?: string, threadOf?: ThreadParent): void {
    this.socket.send(
      encode({ type: "chat", message, conversationId, ...(agentId !== undefined && { agentId }), ...(workdir !== undefined && { workdir }), ...(threadOf !== undefined && { threadOf }) }),
    );
  }

  /** P102 — the folders inside `path` on the hub's machine (or a node's), to pick a conversation's working folder. */
  async listDirs(path?: string): Promise<DirListing> {
    const reply = await this.request((requestId) => ({ type: "listDirs", requestId, ...(path ? { path } : {}) }));
    if (reply.type !== "dirList") return { path: "", dirs: [] };
    return { path: reply.path, parent: reply.parent, dirs: reply.dirs };
  }

  /** P87 — the configured agents' ids, from the hub's settings. */
  async listAgentIds(): Promise<string[]> {
    const reply = await this.request((requestId) => ({ type: "requestSettings", requestId }));
    return reply.type === "settings" ? reply.agentIds : [];
  }

  /** P120, P123 — the agents as the hub's settings hold them (with their roles, superiors and model limits) and the model policies. */
  async listHubAgents(): Promise<HubAgents> {
    const reply = await this.request((requestId) => ({ type: "requestSettings", requestId }));
    if (reply.type !== "settings") throw new Error("unexpected reply from the hub");
    return { agents: reply.agents, modelPolicies: reply.modelPolicies, modelIds: reply.modelIds };
  }

  /** P120 — one change to the agents' organization, with the pairing key. Rejects with `HubRequestError` (`authRejected` on a wrong key). */
  async editAgentOrg(pairingKey: string, edit: OrgEdit): Promise<HubAgents> {
    const reply = await this.request((requestId) => ({ type: "editAgentOrg", requestId, pairingKey, edit }), SAVE_SETTINGS_TIMEOUT_MS);
    if (reply.type !== "settingsSaved") throw new Error("unexpected reply from the hub");
    return { agents: reply.agents, modelPolicies: reply.modelPolicies, modelIds: reply.modelIds };
  }

  /** P123 — the work agents delegated to each other, newest first. */
  async listAgentTasks(): Promise<AgentTask[]> {
    const reply = await this.request((requestId) => ({ type: "listAgentTasks", requestId }));
    if (reply.type !== "agentTaskList") throw new Error("unexpected reply from the hub");
    return reply.tasks;
  }

  /** P123 — pauses, resumes or stops a task running on the hub, with the pairing key. Returns the updated list. Rejects with
   * `HubRequestError` (`authRejected` on a wrong key, or the task doesn't run there). */
  async controlAgentTask(pairingKey: string, taskId: string, action: AgentTaskAction): Promise<AgentTask[]> {
    const reply = await this.request((requestId) => ({ type: "controlAgentTask", requestId, pairingKey, taskId, action }));
    if (reply.type !== "agentTaskList") throw new Error("unexpected reply from the hub");
    return reply.tasks;
  }

  /** P87 — the person's answer to an `approvalRequest`. */
  resolveApproval(approvalId: number, approved: boolean): void {
    this.socket.send(encode({ type: "resolveApproval", approvalId, approved }));
  }

  /** P78 — this device's conversations on the hub, newest-updated first. */
  async listConversations(): Promise<ConversationSummary[]> {
    const reply = await this.request((requestId) => ({ type: "listConversations", requestId }));
    return reply.type === "conversationList" ? reply.conversations : [];
  }

  /** P121 — the id of the conversation that is `agentId`'s channel with this person: always the same, so there is one per agent. It exists on
   * the hub from the first message sent to it. */
  async openAgentChannel(agentId: string): Promise<string> {
    const reply = await this.request((requestId) => ({ type: "openAgentChannel", requestId, agentId }));
    if (reply.type !== "agentChannel") throw new Error("unexpected reply to the agent channel");
    return reply.conversationId;
  }

  async renameConversation(conversationId: string, title: string): Promise<void> {
    await this.request((requestId) => ({ type: "renameConversation", requestId, conversationId, title }));
  }

  async deleteConversation(conversationId: string): Promise<void> {
    await this.request((requestId) => ({ type: "deleteConversation", requestId, conversationId }));
  }

  /** Skills management (P72) — each call is one request/reply pair correlated by `requestId`
   * (the same idea as a ping's nonce, but several can be in flight, so it's a map). */
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

  /** P40 — one of this device's conversations on the hub, oldest first (the most recent `limit`
   * messages). Empty when that conversation doesn't exist yet. */
  async fetchHistory(conversationId: string, limit: number): Promise<HistoryMessage[]> {
    const reply = await this.request((requestId) => ({ type: "requestHistory", requestId, limit, conversationId }));
    return reply.type === "history" ? reply.messages : [];
  }

  private request(build: (requestId: number) => ClientMessage, timeoutMs: number = REQUEST_TIMEOUT_MS): Promise<ServerMessage> {
    return new Promise((resolve, reject) => {
      const requestId = this.nextRequestId++;
      const timeoutId = setTimeout(() => {
        this.pendingRequests.delete(requestId);
        reject(new Error("no response from the server"));
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
