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
  type AgentSettings,
  type ApiKey,
  type Attachment,
  type BotPairingsView,
  type ChatEventDto,
  type ClientMessage,
  type CodeMode,
  type ConversationSummary,
  type HistoryMessage,
  type HubDevice,
  type ProviderEdit,
  type ProviderFallback,
  type SyncAction,
  type SyncStatus,
  type HubSettings,
  type HubSettingsUpdate,
  type LimitStatus,
  type ServerMessage,
  type UsageReport,
  type NodeInfo,
  type NodeFolder,
  type AgentTask,
  type AgentTaskAction,
  type DirListing,
  type ProjectDto,
  type SkillDto,
  type SpaceInfo,
  type Task,
  type TaskInfo,
  type ThreadParent,
  type Webhook,
  type WebhookAuth,
  type WebhookInfo,
  type RemovedUser,
  type UserInfo,
  type VaultSearchHit,
} from "./messages";
import type { OrgEdit } from "./org";

/** The workspace's members (P84) — with the provisional password of the one just created or reset. */
export interface UserList {
  users: UserInfo[];
  tempPassword?: string;
  /** The invite just made for a TruthID link (P84 fatia 5), shown once. */
  inviteCode?: string;
  /** Members removed whose encrypted data is still on the hub; `restoreUser` brings them back. */
  removed: RemovedUser[];
  /** The workspace's recovery policy (P84 fatia 4 parte B): "private", "consent" or "company". */
  recoveryPolicy?: string;
}

/** The hub's scheduled tasks (P92), and whether it runs them on schedule. */
export interface TaskList {
  tasks: TaskInfo[];
  runsHere: boolean;
}

/** The hub's incoming webhooks (P105), and whether it takes their calls. */
export interface WebhookList {
  webhooks: WebhookInfo[];
  servesHere: boolean;
}

/** A webhook's new credential (a token, or a signing secret for an `hmac` one): the only time it exists outside the hub. */
export interface WebhookCreated extends WebhookList {
  id: string;
  credential: string;
  kind: WebhookAuth;
}

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

/** A settings save the hub refused (P78). `conflict`: the file changed since it was loaded.
 * `authRejected`: the pairing key was wrong. */
export class SettingsError extends Error {
  constructor(
    message: string,
    public readonly conflict: boolean,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

/** A Warden API key change the hub refused (P12). `authRejected`: the pairing key was wrong. */
export class ApiKeyError extends Error {
  constructor(
    message: string,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

/** A node access change the hub refused (P93). `authRejected`: the pairing key was wrong. */
export class NodeError extends Error {
  constructor(
    message: string,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

/** A scheduled-task request the hub refused (P92). `authRejected`: the pairing key was wrong. */
export class TaskError extends Error {
  constructor(
    message: string,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

/** A webhook request the hub refused (P105). `authRejected`: the pairing key was wrong, or a member asked. */
export class WebhookError extends Error {
  constructor(
    message: string,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

/** An approve/revoke the hub refused. `authRejected`: the pairing key was wrong. */
export class DeviceError extends Error {
  constructor(
    message: string,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

/** A people request the hub refused (P84). `authRejected`: the pairing key (or, for
 * `changePassword`, the current password) was wrong. */
export class UserError extends Error {
  constructor(
    message: string,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

/** A sync action the hub refused. `authRejected`: the pairing key was wrong. */
export class SyncError extends Error {
  constructor(
    message: string,
    public readonly authRejected: boolean,
  ) {
    super(message);
  }
}

/** The hub's paired devices, and which one is this browser. */
export interface DeviceList {
  devices: HubDevice[];
  you: string;
}

/** The hub's settings as loaded — `version` goes back with the save. */
export interface LoadedSettings {
  settings: HubSettings;
  version: string;
  /** False over plain http:// from another machine: the hub refuses a new API key there. */
  secretsWritable: boolean;
}

/** A note as opened for editing — `version` goes back with the save. */
export interface VaultNote {
  content: string;
  version: string;
}

/** One line of the transcript. `error` entries come from `chatError` or a failed request. */
export interface ChatEntry {
  /** The message's id on the hub (P125), what a thread starts from. Absent on a message not saved yet and on a hub from before threads. */
  id?: string;
  role: "user" | "assistant" | "error";
  content: string;
  attachments: Attachment[];
  /** On an `error` entry: the spending limit the turn stopped on (P4), which can be extended. */
  spendLimitId?: string;
  /** On an `assistant` entry: a reserve answered in place of the turn's provider (P79). */
  fallbacks?: ProviderFallback[];
}

export function historyToEntries(messages: HistoryMessage[]): ChatEntry[] {
  return messages.map((m) => ({ ...(m.id && { id: m.id }), role: m.role, content: m.content, attachments: m.attachments }));
}

type StatusListener = (status: ConnectionStatus) => void;
/** `conversationId` is the conversation the reply belongs to (P78) — the hub always sends it. */
type ChatListener = (entry: ChatEntry, conversationId: string | undefined) => void;

/** P46 — a tool in this browser's turn waiting for the person's yes. */
export interface ApprovalPrompt {
  approvalId: number;
  target: string;
  action: string;
  detail: string;
  /** What "Sempre permitir" would cover (e.g. `git status *`), when this ask can be answered that way (P103 b). */
  always?: string;
  /** P122 — the kind of action this agent has to get approved (an id of `approvalCategories.ts`), when the ask comes from that rule. */
  category?: string;
}

/** `prompt` opens an approval; `cancelled` closes one the hub stopped waiting for. */
export type ApprovalEvent = { kind: "prompt"; prompt: ApprovalPrompt } | { kind: "cancelled"; approvalId: number };
type ApprovalListener = (event: ApprovalEvent) => void;
/** Called with the id of a conversation an agent created or changed (P46 `message_agent`). */
type ConversationsListener = (conversationId: string) => void;
type ChatEventListener = (event: ChatEventDto, conversationId: string) => void;

export interface ConnectOptions {
  url: string;
  deviceId: string;
  deviceName: string;
  /** The hub's pairing key (P36) — only needed until this browser holds a `deviceToken`. */
  authKey: string;
  deviceToken?: string;
  /** P84: pairs as this member instead of with the pairing key — also only until there's a token. */
  username?: string;
  password?: string;
  /** P113: signs in as a member with their TruthID — the hub answers with a QR (`onTruthIdChallenge`) and lets
   * this connection in once the TruthID app approves. */
  truthidLogin?: boolean;
  onTruthIdChallenge?: (payload: string, expiresAtMs: number) => void;
  /** Gives up: closes the socket and rejects the handshake. */
  signal?: AbortSignal;
  handshakeTimeoutMs?: number;
}

/** How long a TruthID sign-in may take: the hub waits two minutes for the phone, which has to create a session on-chain. */
const TRUTHID_HANDSHAKE_TIMEOUT_MS = 130_000;

const DEFAULT_HANDSHAKE_TIMEOUT_MS = 10_000;
const HEARTBEAT_INTERVAL_MS = 20_000;
const REQUEST_TIMEOUT_MS = 15_000;
/** Longer than the hub's own 5-minute pairing wait. */
const SYNC_ACTION_TIMEOUT_MS = 330_000;
const TRANSCRIBE_TIMEOUT_MS = 60_000;
/** A settings save restarts the hub's orchestrator, which starts MCP servers (Tavily via `npx`). */
const SAVE_SETTINGS_TIMEOUT_MS = 120_000;

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
  private readonly approvalListeners = new Set<ApprovalListener>();
  private readonly conversationsListeners = new Set<ConversationsListener>();
  private readonly chatEventListeners = new Set<ChatEventListener>();
  private recoveryCodeListener: ((code: string) => void) | null = null;
  private unclaimedRecoveryCode: string | null = null;
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
    /** P84: the member this browser belongs to — `undefined` for the owner. */
    public readonly user: UserInfo | undefined,
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

  onApproval(listener: ApprovalListener): () => void {
    this.approvalListeners.add(listener);
    return () => this.approvalListeners.delete(listener);
  }

  /** P103 b — the engine's work on a code project's task, as it happens. */
  onChatEvent(listener: ChatEventListener): () => void {
    this.chatEventListeners.add(listener);
    return () => this.chatEventListeners.delete(listener);
  }

  onConversationsChanged(listener: ConversationsListener): () => void {
    this.conversationsListeners.add(listener);
    return () => this.conversationsListeners.delete(listener);
  }

  /** P84 fatia 4: the hub turned encryption on for this member's data when they signed in and sent the
   * recovery code that goes with it. It may have arrived before anyone listened, so the first listener
   * gets it right away. */
  onRecoveryCode(listener: (code: string) => void): () => void {
    this.recoveryCodeListener = listener;
    if (this.unclaimedRecoveryCode !== null) {
      const code = this.unclaimedRecoveryCode;
      this.unclaimedRecoveryCode = null;
      listener(code);
    }
    return () => {
      if (this.recoveryCodeListener === listener) this.recoveryCodeListener = null;
    };
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
        options.signal?.removeEventListener("abort", abort);
        fn();
      };

      let timeoutId: ReturnType<typeof setTimeout> = setTimeout(() => {
        finish(() => {
          socket.close();
          reject(new HandshakeError(`o hub não respondeu em ${timeoutMs / 1000}s`));
        });
      }, timeoutMs);
      const abort = () => finish(() => {
        socket.close();
        reject(new HandshakeError("cancelado"));
      });
      if (options.signal?.aborted) abort();
      else options.signal?.addEventListener("abort", abort, { once: true });

      const onFirstMessage = (event: MessageEvent) => {
        let reply: ServerMessage;
        try {
          reply = decode(event.data as string);
        } catch (err) {
          finish(() => reject(new HandshakeError(err instanceof Error ? err.message : String(err))));
          return;
        }
        // A TruthID sign-in first gets the QR to show, and then waits — much longer than a password would.
        if (reply.type === "truthIdChallenge") {
          options.onTruthIdChallenge?.(reply.payload, reply.expiresAtMs);
          clearTimeout(timeoutId);
          timeoutId = setTimeout(() => {
            finish(() => {
              socket.close();
              reject(new HandshakeError("o login com TruthID demorou demais — tente de novo"));
            });
          }, TRUTHID_HANDSHAKE_TIMEOUT_MS);
          return;
        }
        finish(() => {
          switch (reply.type) {
            case "helloAck":
              resolve(new ServerConnection(socket, reply.serverName, reply.deviceToken, reply.user));
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
            ...(options.username !== undefined && { username: options.username, password: options.password ?? "" }),
            ...(options.truthidLogin && { truthidLogin: true }),
            // This UI shows a member's recovery code (RecoveryCodeView), so the hub may encrypt their data.
            recoveryCodes: true,
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
          listener(
            { role: "assistant", content: message.content, attachments: message.attachments, ...(message.fallbacks.length > 0 && { fallbacks: message.fallbacks }) },
            message.conversationId,
          );
        }
        break;
      case "chatError":
        for (const listener of this.chatListeners) {
          listener({ role: "error", content: message.message, attachments: [], spendLimitId: message.spendLimitId }, message.conversationId);
        }
        break;
      case "chatEvent":
        for (const listener of this.chatEventListeners) listener(message.event, message.conversationId);
        break;
      case "approvalRequest": {
        const { approvalId, target, action, detail, always, category } = message;
        for (const listener of this.approvalListeners) listener({ kind: "prompt", prompt: { approvalId, target, action, detail, always, category } });
        break;
      }
      case "approvalCancelled":
        for (const listener of this.approvalListeners) listener({ kind: "cancelled", approvalId: message.approvalId });
        break;
      case "conversationsChanged":
        for (const listener of this.conversationsListeners) listener(message.conversationId);
        break;
      case "toolCallRequest":
        // Never advertised any tools, so the hub shouldn't ask — answer anyway so it isn't left waiting.
        this.socket.send(encode({ type: "toolCallError", callId: message.callId, message: "the web UI has no local tools" }));
        break;
      case "skillList":
      case "skillOk":
      case "projectList":
      case "projectOk":
      case "dirList":
      case "history":
      case "conversationList":
      case "conversationOk":
      case "transcription":
      case "vaultFileList":
      case "vaultNote":
      case "vaultSaved":
      case "vaultOk":
      case "vaultSearchResults":
      case "usageReport":
      case "limitExtended":
      case "settings":
      case "settingsSaved":
      case "deviceList":
      case "apiKeyList":
      case "apiKeyCreated":
      case "syncStatus":
      case "taskList":
      case "agentTaskList":
      case "webhookList":
      case "webhookCreated":
      case "nodeList":
      case "userList":
      case "spaceList":
      case "passwordChanged":
      case "recoveryPolicy":
      case "recoveryPolicyAccepted":
      case "recoveryNoticesAcked":
      case "learningSet":
      case "botPairings":
      case "providerTest":
      case "truthIdLinked":
        this.settleRequest(message.requestId, (pending) => pending.resolve(message));
        break;
      case "recoveryCode":
        if (message.requestId === 0) {
          // Sent by the hub on its own, right after the handshake — the screen may not be listening yet.
          const listener = this.recoveryCodeListener;
          if (listener) listener(message.code);
          else this.unclaimedRecoveryCode = message.code;
        } else {
          this.settleRequest(message.requestId, (pending) => pending.resolve(message));
        }
        break;
      case "userError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new UserError(message.message, message.authRejected)));
        break;
      case "nodeError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new NodeError(message.message, message.authRejected)));
        break;
      case "taskError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new TaskError(message.message, message.authRejected)));
        break;
      case "webhookError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new WebhookError(message.message, message.authRejected)));
        break;
      case "apiKeyError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new ApiKeyError(message.message, message.authRejected)));
        break;
      case "syncError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new SyncError(message.message, message.authRejected)));
        break;
      case "deviceError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new DeviceError(message.message, message.authRejected)));
        break;
      case "settingsError":
        this.settleRequest(message.requestId, (pending) =>
          pending.reject(new SettingsError(message.message, message.conflict, message.authRejected)),
        );
        break;
      case "usageError":
        this.settleRequest(message.requestId, (pending) => pending.reject(new Error(message.message)));
        break;
      case "vaultError":
        this.settleRequest(message.requestId, (pending) =>
          pending.reject(message.conflict ? new VaultConflictError(message.message) : new Error(message.message)),
        );
        break;
      case "skillError":
      case "projectError":
      case "dirError":
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
   * images/PDFs, spoken by `agentId` (P46) when set. `projectId` (P103) is the project a *new* conversation starts in;
   * the hub ignores it for one that exists. The reply arrives asynchronously via `onChatMessage`, tagged with the same id. */
  /** P103 b — asks the hub to stop the task this conversation is running. The turn still ends with `onChatMessage`. */
  cancelTurn(conversationId: string): void {
    this.socket.send(encode({ type: "cancelTurn", conversationId }));
  }

  /** P103 b — how much this code conversation asks; the hub forgets it on a restart, so it is said again with each task. */
  setCodeMode(conversationId: string, mode: CodeMode): void {
    this.socket.send(encode({ type: "setCodeMode", conversationId, mode }));
  }

  sendChat(message: string, conversationId: string, attachments: Attachment[] = [], agentId?: string, projectId?: string, workdir?: string, threadOf?: ThreadParent): void {
    this.socket.send(
      encode({
        type: "chat",
        message,
        conversationId,
        ...(attachments.length > 0 && { attachments }),
        ...(agentId && { agentId }),
        ...(projectId && { projectId }),
        ...(workdir && { workdir }),
        // P125: only the turn that starts a thread says what it is a thread of; the hub keeps that for the life of the conversation.
        ...(threadOf && { threadOf }),
      }),
    );
  }

  /** P102 — the folders inside `path` on the hub's machine, to pick a working folder; no `path` starts where the person may. */
  async listDirs(path?: string): Promise<DirListing> {
    const reply = await this.request((requestId) => ({ type: "listDirs", requestId, ...(path ? { path } : {}) }));
    if (reply.type !== "dirList") return { path: "", dirs: [] };
    return { path: reply.path, parent: reply.parent, dirs: reply.dirs };
  }

  /** P46 — the person's answer to an `ApprovalPrompt`. */
  resolveApproval(approvalId: number, approved: boolean, always = false): void {
    this.socket.send(encode({ type: "resolveApproval", approvalId, approved, ...(always ? { always } : {}) }));
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

  /** P103 — puts a conversation in a project, or out of any (`undefined`). The next turn runs in the new scope. */
  async moveConversation(conversationId: string, projectId?: string): Promise<void> {
    await this.request((requestId) => ({ type: "moveConversation", requestId, conversationId, ...(projectId && { projectId }) }));
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

  /** P103 — the projects of the person's own vault, by name. */
  async listProjects(): Promise<ProjectDto[]> {
    const reply = await this.request((requestId) => ({ type: "listProjects", requestId }));
    return reply.type === "projectList" ? reply.projects : [];
  }

  async saveProject(project: ProjectDto, overwrite: boolean): Promise<void> {
    await this.request((requestId) => ({ type: "saveProject", requestId, project, overwrite }));
  }

  /** Removes only the project's `PROJECT.md`: its files stay in the vault, and its conversations go on without a project. */
  async deleteProject(id: string): Promise<void> {
    await this.request((requestId) => ({ type: "deleteProject", requestId, id }));
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

  /** Tokens across every device on the hub, plus spending limits and recent dollars (P78). */
  async requestUsage(): Promise<UsageReport> {
    // `getTimezoneOffset` is minutes *behind* UTC (180 at UTC−3); the hub wants the offset itself.
    const tzOffsetMinutes = -new Date().getTimezoneOffset();
    const reply = await this.request((requestId) => ({ type: "requestUsage", requestId, tzOffsetMinutes }));
    if (reply.type !== "usageReport") throw new Error("resposta inesperada do hub");
    return reply.report;
  }

  /** Lets a spending limit go one step further for the rest of its window; returns where it stands. */
  async extendLimit(limitId: string): Promise<LimitStatus> {
    const reply = await this.request((requestId) => ({ type: "extendLimit", requestId, limitId }));
    if (reply.type !== "limitExtended") throw new Error("resposta inesperada do hub");
    return reply.limit;
  }

  /** The part of the hub's config the web edits (P78) — never any API key, only whether one is set. */
  async requestSettings(): Promise<LoadedSettings> {
    const reply = await this.request((requestId) => ({ type: "requestSettings", requestId }));
    if (reply.type !== "settings") throw new Error("resposta inesperada do hub");
    return { settings: reply.settings, version: reply.version, secretsWritable: reply.secretsWritable };
  }

  /** Replaces the editable settings and restarts the hub's orchestrator with them. Rejects with
   * `SettingsError` on a wrong pairing key, a conflict, or settings the hub can't start with. */
  async saveSettings(pairingKey: string, baseVersion: string, update: HubSettingsUpdate): Promise<{ settings: HubSettings; version: string }> {
    const reply = await this.request(
      (requestId) => ({ type: "saveSettings", requestId, pairingKey, baseVersion, update }),
      SAVE_SETTINGS_TIMEOUT_MS,
    );
    if (reply.type !== "settingsSaved") throw new Error("resposta inesperada do hub");
    return { settings: reply.settings, version: reply.version };
  }

  /** Uma mudança na organização dos agentes, pela árvore (P120): um cargo e superior, um subordinado novo, uma remoção. Pede a chave de
   * pareamento e reinicia o orquestrador do hub com a mudança; devolve os agentes de agora. Rejeita com `SettingsError` (chave errada, um
   * ciclo, um nome já usado...). */
  async editAgentOrg(pairingKey: string, edit: OrgEdit): Promise<AgentSettings[]> {
    const reply = await this.request((requestId) => ({ type: "editAgentOrg", requestId, pairingKey, edit }), SAVE_SETTINGS_TIMEOUT_MS);
    if (reply.type !== "settingsSaved") throw new Error("resposta inesperada do hub");
    return reply.settings.agents;
  }

  /** Every device that has ever connected to the hub (Sessão 103). */
  async listDevices(): Promise<DeviceList> {
    const reply = await this.request((requestId) => ({ type: "listDevices", requestId }));
    if (reply.type !== "deviceList") throw new Error("resposta inesperada do hub");
    return { devices: reply.devices, you: reply.you };
  }

  /** The Warden API's keys (P12), oldest first. */
  async listApiKeys(): Promise<ApiKey[]> {
    const reply = await this.request((requestId) => ({ type: "listApiKeys", requestId }));
    if (reply.type !== "apiKeyList") throw new Error("resposta inesperada do hub");
    return reply.keys;
  }

  /** A new key: `key` is the only time it's ever sent. Rejects with `ApiKeyError`. */
  async createApiKey(pairingKey: string, name: string, agentId?: string): Promise<{ key: string; keys: ApiKey[] }> {
    const reply = await this.request((requestId) => ({ type: "createApiKey", requestId, pairingKey, name, ...(agentId && { agentId }) }));
    if (reply.type !== "apiKeyCreated") throw new Error("resposta inesperada do hub");
    return { key: reply.key, keys: reply.keys };
  }

  async revokeApiKey(pairingKey: string, id: string): Promise<ApiKey[]> {
    const reply = await this.request((requestId) => ({ type: "revokeApiKey", requestId, pairingKey, id }));
    if (reply.type !== "apiKeyList") throw new Error("resposta inesperada do hub");
    return reply.keys;
  }

  /** Nodes (P93): connected or allowed, with what each offers and who may use it. */
  async listNodes(): Promise<NodeInfo[]> {
    const reply = await this.request((requestId) => ({ type: "listNodes", requestId }));
    if (reply.type !== "nodeList") throw new Error("resposta inesperada do hub");
    return reply.nodes;
  }

  /** Writes a node's access; rejects with `NodeError` (wrong pairing key: `authRejected`). */
  async setNodeAccess(pairingKey: string, deviceId: string, enabled: boolean, agents: string[], requireApproval: boolean): Promise<NodeInfo[]> {
    const reply = await this.request((requestId) => ({ type: "setNodeAccess", requestId, pairingKey, deviceId, enabled, agents, requireApproval }));
    if (reply.type !== "nodeList") throw new Error("resposta inesperada do hub");
    return reply.nodes;
  }

  /** O trabalho que os agentes passaram uns aos outros em segundo plano (P123), do mais novo ao mais antigo. Só leitura. */
  async listAgentTasks(): Promise<AgentTask[]> {
    const reply = await this.request((requestId) => ({ type: "listAgentTasks", requestId }));
    if (reply.type !== "agentTaskList") throw new Error("resposta inesperada do hub");
    return reply.tasks;
  }

  /** Pausa, retoma ou para uma tarefa delegada que roda no hub (P123), com as subtarefas abaixo dela. Pede a chave de pareamento; rejeita
   * com `TaskError` (chave errada, ou a tarefa não roda aí). Devolve a lista atualizada. */
  async controlAgentTask(pairingKey: string, taskId: string, action: AgentTaskAction): Promise<AgentTask[]> {
    const reply = await this.request((requestId) => ({ type: "controlAgentTask", requestId, pairingKey, taskId, action }));
    if (reply.type !== "agentTaskList") throw new Error("resposta inesperada do hub");
    return reply.tasks;
  }

  /** Scheduled tasks (P92), in the config's order, and whether this hub runs them on schedule. */
  async listTasks(): Promise<TaskList> {
    return this.taskRequest((requestId) => ({ type: "listTasks", requestId }));
  }

  /** Creates a task, or replaces `originalId` with it. Every change rejects with `TaskError`. */
  async saveTask(pairingKey: string, task: Task, originalId?: string): Promise<TaskList> {
    return this.taskRequest((requestId) => ({ type: "saveTask", requestId, pairingKey, task, ...(originalId && { originalId }) }));
  }

  async setTaskEnabled(pairingKey: string, id: string, enabled: boolean): Promise<TaskList> {
    return this.taskRequest((requestId) => ({ type: "setTaskEnabled", requestId, pairingKey, id, enabled }));
  }

  async deleteTask(pairingKey: string, id: string): Promise<TaskList> {
    return this.taskRequest((requestId) => ({ type: "deleteTask", requestId, pairingKey, id }));
  }

  /** Starts a run on the hub; `onConversationsChanged` with `task-<id>` tells when it's done. */
  async runTask(pairingKey: string, id: string): Promise<TaskList> {
    return this.taskRequest((requestId) => ({ type: "runTask", requestId, pairingKey, id }));
  }

  private async taskRequest(build: (requestId: number) => ClientMessage): Promise<TaskList> {
    const reply = await this.request(build);
    if (reply.type !== "taskList") throw new Error("resposta inesperada do hub");
    return { tasks: reply.tasks, runsHere: reply.runsHere };
  }

  /** Incoming webhooks (P105), in the config's order, and whether this hub takes their calls. Owner only. */
  async listWebhooks(): Promise<WebhookList> {
    return this.webhookRequest((requestId) => ({ type: "listWebhooks", requestId }));
  }

  /** Creates a webhook, or replaces `originalId` with it (a rename keeps its credential; a change of `auth` drops it).
   * Every change rejects with `WebhookError`. */
  async saveWebhook(pairingKey: string, webhook: Webhook, originalId?: string): Promise<WebhookList> {
    return this.webhookRequest((requestId) => ({ type: "saveWebhook", requestId, pairingKey, webhook, ...(originalId && { originalId }) }));
  }

  async setWebhookEnabled(pairingKey: string, id: string, enabled: boolean): Promise<WebhookList> {
    return this.webhookRequest((requestId) => ({ type: "setWebhookEnabled", requestId, pairingKey, id, enabled }));
  }

  async deleteWebhook(pairingKey: string, id: string): Promise<WebhookList> {
    return this.webhookRequest((requestId) => ({ type: "deleteWebhook", requestId, pairingKey, id }));
  }

  /** A new credential for a webhook — a token, or a signing secret when it wants `hmac` — replacing the old one. It comes
   * back once, here. */
  async createWebhookCredential(pairingKey: string, id: string): Promise<WebhookCreated> {
    const reply = await this.request((requestId) => ({ type: "createWebhookCredential", requestId, pairingKey, id }));
    if (reply.type !== "webhookCreated") throw new Error("resposta inesperada do hub");
    return { id: reply.id, credential: reply.credential, kind: reply.kind, webhooks: reply.webhooks, servesHere: reply.servesHere };
  }

  /** Takes a webhook's credential away; its calls get 401 from the next one on. */
  async revokeWebhookCredential(pairingKey: string, id: string): Promise<WebhookList> {
    return this.webhookRequest((requestId) => ({ type: "revokeWebhookCredential", requestId, pairingKey, id }));
  }

  private async webhookRequest(build: (requestId: number) => ClientMessage): Promise<WebhookList> {
    const reply = await this.request(build);
    if (reply.type !== "webhookList") throw new Error("resposta inesperada do hub");
    return { webhooks: reply.webhooks, servesHere: reply.servesHere };
  }

  /** P84: the member on this connection picks their own password; rejects with `UserError`
   * (`authRejected`: the current one was wrong). */
  async changePassword(oldPassword: string, newPassword: string, recoveryCode?: string): Promise<{ recoveryCode?: string }> {
    const reply = await this.request((requestId) => ({
      type: "changePassword",
      requestId,
      oldPassword,
      newPassword,
      ...(recoveryCode && { recoveryCode }),
    }));
    if (reply.type !== "passwordChanged") throw new Error("resposta inesperada do hub");
    return { ...(reply.recoveryCode !== undefined && { recoveryCode: reply.recoveryCode }) };
  }

  /** P84 fatia 4: a new recovery code, shown once; the old one stops working. Rejects with `UserError`
   * (`authRejected`: the password was wrong). */
  async regenerateRecoveryCode(password: string): Promise<string> {
    const reply = await this.request((requestId) => ({ type: "regenerateRecoveryCode", requestId, password }));
    if (reply.type !== "recoveryCode") throw new Error("resposta inesperada do hub");
    return reply.code;
  }

  /** P84: the workspace's members — the owner's only. */
  async listUsers(): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "listUsers", requestId }));
  }

  /** Creates a member (`isNew`, the reply carries their provisional password once) or renames one. */
  async saveUser(pairingKey: string, id: string, name: string, isNew: boolean): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "saveUser", requestId, pairingKey, id, name, isNew }));
  }

  /** A new provisional password for a member, in the reply once. */
  async resetPassword(pairingKey: string, id: string): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "resetPassword", requestId, pairingKey, id }));
  }

  /** Brings back a removed member whose encrypted data was kept, with the password they had. */
  async restoreUser(pairingKey: string, id: string): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "restoreUser", requestId, pairingKey, id }));
  }

  /** P84 fatia 5: an invite for a member to link their TruthID, in the reply once. */
  async createInvite(pairingKey: string, id: string): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "createInvite", requestId, pairingKey, id }));
  }

  /** Unties a member's TruthID and cancels an open invite. */
  async unlinkTruthId(pairingKey: string, id: string): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "unlinkTruthId", requestId, pairingKey, id }));
  }

  /** A member links their TruthID with the owner's invite. Answers with the username the registry has. */
  async redeemInvite(code: string, username: string): Promise<string> {
    const reply = await this.request((requestId) => ({ type: "redeemInvite", requestId, code, username }));
    if (reply.type !== "truthIdLinked") throw new Error("resposta inesperada do hub");
    return reply.username;
  }

  /** Removes a member and revokes their devices; their vault and conversations stay on the hub. */
  async removeUser(pairingKey: string, id: string): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "removeUser", requestId, pairingKey, id }));
  }

  /** P84 fatia 2: the tools a member may use (`null`: the safe default). Rejects with `UserError`. */
  async setUserTools(pairingKey: string, id: string, tools: string[] | null): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "setUserTools", requestId, pairingKey, id, tools }));
  }

  /** P102: the folders a member may pick as a working folder — of the hub's machine and on nodes. Rejects with `UserError`
   * for a path that isn't valid. */
  async setUserWorkdirs(pairingKey: string, id: string, workdirs: string[], nodeWorkdirs: NodeFolder[]): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "setUserWorkdirs", requestId, pairingKey, id, workdirs, nodeWorkdirs }));
  }

  /** P115: the model the assistant's learning uses for a member (`null`: the workspace's). */
  async setUserLearningProvider(pairingKey: string, id: string, provider: string | null): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "setUserLearningProvider", requestId, pairingKey, id, provider }));
  }

  /** A member creates (`originalId` absent) or edits one of their own agents; answers with their
   * settings view. Rejects with `SettingsError`. */
  async saveOwnAgent(agent: AgentSettings, originalId?: string): Promise<LoadedSettings> {
    const reply = await this.request((requestId) => ({ type: "saveOwnAgent", requestId, agent, ...(originalId && { originalId }) }));
    if (reply.type !== "settings") throw new Error("resposta inesperada do hub");
    return { settings: reply.settings, version: reply.version, secretsWritable: reply.secretsWritable };
  }

  async deleteOwnAgent(id: string): Promise<LoadedSettings> {
    const reply = await this.request((requestId) => ({ type: "deleteOwnAgent", requestId, id }));
    if (reply.type !== "settings") throw new Error("resposta inesperada do hub");
    return { settings: reply.settings, version: reply.version, secretsWritable: reply.secretsWritable };
  }

  /** P84 fatia 3: the shared spaces — every one for the owner, a member's own for them. */
  async listSpaces(): Promise<SpaceInfo[]> {
    return this.spaceRequest((requestId) => ({ type: "listSpaces", requestId }));
  }

  /** The owner shares a folder (`originalId` absent) or changes a space. Rejects with `UserError`. */
  async saveSpace(pairingKey: string, space: SpaceInfo, originalId?: string): Promise<SpaceInfo[]> {
    return this.spaceRequest((requestId) => ({ type: "saveSpace", requestId, pairingKey, space, ...(originalId && { originalId }) }));
  }

  /** Stops sharing a folder; it stays in the owner's vault. */
  async deleteSpace(pairingKey: string, id: string): Promise<SpaceInfo[]> {
    return this.spaceRequest((requestId) => ({ type: "deleteSpace", requestId, pairingKey, id }));
  }

  /** P117: the strangers waiting to talk to the Telegram or WhatsApp bot (the owner's connection only). */
  async listBotPairings(): Promise<BotPairingsView> {
    return this.botPairingRequest((requestId) => ({ type: "listBotPairings", requestId }));
  }

  /** The owner lets the sender behind `code` in (`approve`) or drops the request; answers with what is still
   * waiting. `member` approves the chat as speaking as that member of the workspace (they have to be linked to the
   * bots), otherwise as the owner. Rejects with `UserError`. */
  async resolveBotPairing(pairingKey: string, code: string, approve: boolean, member?: string): Promise<BotPairingsView> {
    return this.botPairingRequest((requestId) => ({ type: "resolveBotPairing", requestId, pairingKey, code, approve, ...(member && { member }) }));
  }

  /** P10: checks a provider's key without spending a conversation (the owner's connection, with the pairing key).
   * `provider` is the form as the screen has it; the hub only asks an address it has saved. Rejects with `UserError`
   * (a wrong pairing key, an address to save first, a key typed over a connection that isn't encrypted). */
  async testProvider(pairingKey: string, provider: ProviderEdit): Promise<{ ok: boolean; kind: string; message: string }> {
    const reply = await this.request((requestId) => ({ type: "testProvider", requestId, pairingKey, provider }));
    if (reply.type !== "providerTest") throw new Error("resposta inesperada do hub");
    return { ok: reply.ok, kind: reply.kind, message: reply.message };
  }

  private async botPairingRequest(build: (requestId: number) => ClientMessage): Promise<BotPairingsView> {
    const reply = await this.request(build);
    if (reply.type !== "botPairings") throw new Error("resposta inesperada do hub");
    // A hub from before the choice of member sends none: nobody can be chosen, the owner is what there was.
    return { pairings: reply.pairings, members: reply.members ?? [] };
  }

  private async spaceRequest(build: (requestId: number) => ClientMessage): Promise<SpaceInfo[]> {
    const reply = await this.request(build);
    if (reply.type !== "spaceList") throw new Error("resposta inesperada do hub");
    return reply.spaces;
  }

  private async userRequest(build: (requestId: number) => ClientMessage): Promise<UserList> {
    const reply = await this.request(build);
    if (reply.type !== "userList") throw new Error("resposta inesperada do hub");
    return {
      users: reply.users,
      ...(reply.tempPassword !== undefined && { tempPassword: reply.tempPassword }),
      ...(reply.inviteCode !== undefined && { inviteCode: reply.inviteCode }),
      removed: reply.removed ?? [],
      ...(reply.recoveryPolicy !== undefined && { recoveryPolicy: reply.recoveryPolicy }),
    };
  }

  /** P84 fatia 4 parte B: the owner sets the workspace's recovery policy. `secret` is the owner's recovery
   * key, only when one was just made — shown once, never kept. Rejects with `UserError`. */
  async setRecoveryPolicy(pairingKey: string, policy: string, newKey = false): Promise<{ policy: string; secret?: string }> {
    const reply = await this.request((requestId) => ({ type: "setRecoveryPolicy", requestId, pairingKey, policy, newKey }));
    if (reply.type !== "recoveryPolicy") throw new Error("resposta inesperada do hub");
    return { policy: reply.policy, ...(reply.secret !== undefined && { secret: reply.secret }) };
  }

  /** The owner opens a member's data with the workspace's recovery key (`consent` also needs the person's
   * `code`) and gives them a new provisional password (`tempPassword` in the answer, shown once). */
  async recoverMember(pairingKey: string, id: string, recoveryKey: string, code?: string): Promise<UserList> {
    return this.userRequest((requestId) => ({ type: "recoverMember", requestId, pairingKey, id, recoveryKey, ...(code && { code }) }));
  }

  /** A member says yes to a weaker recovery policy. `recoveryCode`: entering or leaving "consent" made a new one. */
  async acceptRecoveryPolicy(password: string): Promise<{ recoveryCode?: string }> {
    const reply = await this.request((requestId) => ({ type: "acceptRecoveryPolicy", requestId, password }));
    if (reply.type !== "recoveryPolicyAccepted") throw new Error("resposta inesperada do hub");
    return { ...(reply.recoveryCode !== undefined && { recoveryCode: reply.recoveryCode }) };
  }

  /** A member has seen the recoveries the owner made. */
  /** P115: a member's own choice — `enabled: false` stops the assistant learning from their conversations.
   * Rejects with `UserError` (the owner has no such switch over the wire). */
  async setLearning(enabled: boolean): Promise<void> {
    const reply = await this.request((requestId) => ({ type: "setLearning", requestId, enabled }));
    if (reply.type !== "learningSet") throw new Error("resposta inesperada do hub");
  }

  async ackRecoveryNotices(): Promise<void> {
    const reply = await this.request((requestId) => ({ type: "ackRecoveryNotices", requestId }));
    if (reply.type !== "recoveryNoticesAcked") throw new Error("resposta inesperada do hub");
  }

  /** Approves or revokes a device; rejects with `DeviceError` on a wrong pairing key. */
  async setDeviceStatus(pairingKey: string, deviceId: string, action: "approve" | "revoke"): Promise<DeviceList> {
    const reply = await this.request((requestId) => ({ type: "setDeviceStatus", requestId, pairingKey, deviceId, action }));
    if (reply.type !== "deviceList") throw new Error("resposta inesperada do hub");
    return { devices: reply.devices, you: reply.you };
  }

  /** Where the hub's vault syncs to and how its last round went (P61). */
  async requestSyncStatus(): Promise<SyncStatus> {
    const reply = await this.request((requestId) => ({ type: "requestSyncStatus", requestId }));
    if (reply.type !== "syncStatus") throw new Error("resposta inesperada do hub");
    return reply.status;
  }

  /** Runs a sync round or sets up the hub's vault key; rejects with `SyncError`. A round over the
   * network or a pairing can take minutes, hence the longer wait. */
  async syncAction(pairingKey: string, action: SyncAction): Promise<SyncStatus> {
    const reply = await this.request((requestId) => ({ type: "syncAction", requestId, pairingKey, action }), SYNC_ACTION_TIMEOUT_MS);
    if (reply.type !== "syncStatus") throw new Error("resposta inesperada do hub");
    return reply.status;
  }

  /** Has the hub show a pairing code (P88); asking again while one is up gives the same code. */
  async startPairHost(pairingKey: string): Promise<{ status: SyncStatus; code: string }> {
    const reply = await this.request((requestId) => ({ type: "syncAction", requestId, pairingKey, action: { kind: "pairHost" } }));
    if (reply.type !== "syncStatus" || !reply.pairingCode) throw new Error("resposta inesperada do hub");
    return { status: reply.status, code: reply.pairingCode };
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
