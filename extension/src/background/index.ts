/**
 * Background service worker (Fase 8.1/8.2) — owns the single `ServerConnection` instance and
 * this session's chat history. Lives here (never in the side panel, which the user can close
 * independently) so the connection survives the panel opening/closing repeatedly while the user
 * chats.
 *
 * `history` is in-memory only, not persisted to `chrome.storage` — the hub already keeps this
 * device's conversations (P40, several since P78), so every connect reloads the open one from there
 * (`loadHistory`); a service worker that gets evicted loses the connection and the transcript
 * together, and the next connect brings the transcript back. Only which conversation was open is
 * kept in `chrome.storage`, so it reopens on the next connect. No automatic
 * reconnect either: a fresh service worker reports `disconnected`, and the panel shows the
 * connection form again — same accepted gap `server_connection.dart` (Fase 7.2) drew, see its doc
 * comment.
 */

import { HubRequestError, ServerConnection, type ChatEntry, type ConnectionStatus } from "./connection";
import { discoverHubs } from "./discovery";
import type { ConnectionSettings, ConversationState, PopupRequest } from "./popup_protocol";
import type { ApprovalPrompt, ConversationSummary } from "../protocol/messages";
import { toolSpecs, toolHandlers } from "./tools";
import { addActiveTabToGroup, listGroupTabs, removeTabFromGroup, setGroupChangeListener } from "./tab_group";
import { setUpPanelOpening, supportsHubDiscovery } from "./platform";

// Makes clicking the toolbar icon open the chat panel (Chrome's side panel, Firefox's sidebar)
// instead of requiring a `default_popup`. Without this call the icon click has no effect.
setUpPanelOpening();

const STORAGE_KEY_DEVICE_ID = "deviceId";
const STORAGE_KEY_SETTINGS = "connectionSettings";
const STORAGE_KEY_DEVICE_TOKENS = "deviceTokens";
const STORAGE_KEY_ACTIVE_CONVERSATION = "activeConversation";

let connection: ServerConnection | null = null;
/** The open conversation's transcript. */
let history: ChatEntry[] = [];
let currentStatus: ConnectionStatus = { kind: "disconnected" };
/** P78 — this device's conversations on the hub, and the one the chat shows. */
let conversations: ConversationSummary[] = [];
let activeConversationId: string | null = null;
/** Turns sent and not answered yet, by conversation id — the hub only saves a turn once it's
 * answered, so this is what puts the question back on screen when switching to that conversation. */
let pendingTurns: Record<string, string> = {};
/** P87 — the hub's configured agents and the one the next turn speaks as. */
let agentIds: string[] = [];
let agentId: string | null = null;
/** P102 — the open conversation's working folder, or the one a new conversation will start in. Chosen before its first
 * message and fixed after (the hub keeps the one it was created with), so this only matters while it is unsent. */
let workdir: string | null = null;
/** P87 — approvals the hub is waiting on. The panel may be closed, so the icon shows a "!" too. */
let approvals: ApprovalPrompt[] = [];

async function getOrCreateDeviceId(): Promise<string> {
  const stored = await chrome.storage.local.get(STORAGE_KEY_DEVICE_ID);
  const existing = stored[STORAGE_KEY_DEVICE_ID] as string | undefined;
  if (existing) return existing;
  const generated = crypto.randomUUID();
  await chrome.storage.local.set({ [STORAGE_KEY_DEVICE_ID]: generated });
  return generated;
}

/** P36 — device tokens per hub (`host:port`), since this extension has a single device id. Kept
 * apart from `connectionSettings` so they never travel to the panel with `getStatus`. */
async function getDeviceTokens(): Promise<Record<string, string>> {
  const stored = await chrome.storage.local.get(STORAGE_KEY_DEVICE_TOKENS);
  return (stored[STORAGE_KEY_DEVICE_TOKENS] as Record<string, string> | undefined) ?? {};
}

async function saveDeviceToken(hub: string, token: string): Promise<void> {
  await chrome.storage.local.set({ [STORAGE_KEY_DEVICE_TOKENS]: { ...(await getDeviceTokens()), [hub]: token } });
}

async function getSavedSettings(): Promise<Partial<ConnectionSettings>> {
  const stored = await chrome.storage.local.get(STORAGE_KEY_SETTINGS);
  return (stored[STORAGE_KEY_SETTINGS] as ConnectionSettings | undefined) ?? {};
}

/** Best-effort broadcast to the side panel if it's currently open and listening — a closed panel
 * means no receiver, which `sendMessage` reports as a rejected promise; that's the expected common
 * case, not an error worth surfacing. */
function broadcast(message: unknown): void {
  chrome.runtime.sendMessage(message).catch(() => {});
}

function setStatus(status: ConnectionStatus): void {
  currentStatus = status;
  broadcast({ type: "statusChanged", status });
}

function addChatEntry(entry: ChatEntry): void {
  history.push(entry);
  broadcast({ type: "chatMessage", entry });
}

/** How many past messages to show when a conversation opens — same cut as the mobile app (P40). */
const HISTORY_LIMIT = 100;

function conversationState(): ConversationState {
  return { conversations, activeConversationId, pendingIds: Object.keys(pendingTurns), agentIds, agentId, workdir };
}

function setApprovals(next: ApprovalPrompt[]): void {
  approvals = next;
  void chrome.action.setBadgeText({ text: approvals.length > 0 ? "!" : "" });
  broadcast({ type: "approvalsChanged", approvals });
}

/** P87 — re-reads the configured agents. A hub that can't answer leaves the selector empty. */
async function refreshAgents(from: ServerConnection): Promise<void> {
  let ids: string[];
  try {
    ids = await from.listAgentIds();
  } catch (err) {
    console.warn("warden: could not list agents", err);
    ids = [];
  }
  if (connection !== from) return;
  agentIds = ids;
  broadcastConversations();
}

/** The agent `id` last spoke with, when the hub has that conversation; otherwise the choice stays. */
function restoreAgent(id: string): void {
  const conversation = conversations.find((c) => c.id === id);
  if (conversation) agentId = conversation.agentId ?? null;
  // The folder is the conversation's own: one that isn't on the hub yet starts with none.
  workdir = conversation?.workdir ?? null;
}

function broadcastConversations(): void {
  broadcast({ type: "conversationsChanged", ...conversationState() });
}

function setActiveConversation(id: string): void {
  activeConversationId = id;
  void chrome.storage.local.set({ [STORAGE_KEY_ACTIVE_CONVERSATION]: id });
}

/** P78 — re-reads the conversation list. A conversation started here whose first turn is still
 * in flight isn't on the hub yet, so it stays in the list until it is. */
async function refreshConversations(from: ServerConnection): Promise<ConversationSummary[] | undefined> {
  let list: ConversationSummary[];
  try {
    list = await from.listConversations();
  } catch (err) {
    console.warn("warden: could not list conversations", err);
    return undefined;
  }
  if (connection !== from) return undefined;
  conversations = [...conversations.filter((c) => c.id in pendingTurns && !list.some((l) => l.id === c.id)), ...list];
  broadcastConversations();
  return list;
}

/** P40 — the open conversation's transcript from the hub, plus its unanswered turn if there is
 * one. Runs after `connect` returns, so a slow history never delays the panel showing the chat. */
async function loadHistory(from: ServerConnection, conversationId: string): Promise<void> {
  let loaded: ChatEntry[];
  try {
    loaded = (await from.fetchHistory(conversationId, HISTORY_LIMIT)).map((m) => ({ role: m.role, content: m.content }));
  } catch (err) {
    loaded = [{ role: "error", content: `Could not load earlier messages: ${err instanceof Error ? err.message : String(err)}` }];
  }
  // Disconnected, reconnected elsewhere or switched conversations meanwhile.
  if (connection !== from || activeConversationId !== conversationId) return;
  const waiting = pendingTurns[conversationId];
  history = waiting === undefined ? loaded : [...loaded, { role: "user", content: waiting }];
  broadcast({ type: "historyLoaded", history });
}

/** P78 — shows `id` in the chat: empty right away, then its transcript once the hub answers. */
function openConversation(id: string): void {
  setActiveConversation(id);
  restoreAgent(id);
  history = [];
  broadcast({ type: "historyLoaded", history });
  broadcastConversations();
  if (connection) void loadHistory(connection, id);
}

/** P78 — on connect: the list, then the conversation that was open last time (or the most recent,
 * if that one was deleted elsewhere; a fresh one when there are none). */
async function restoreConversations(from: ServerConnection): Promise<void> {
  const list = await refreshConversations(from);
  if (connection !== from) return;
  const current = activeConversationId;
  if (list && (current === null || !list.some((c) => c.id === current))) {
    setActiveConversation(list[0]?.id ?? crypto.randomUUID());
  }
  if (activeConversationId !== null) restoreAgent(activeConversationId);
  broadcastConversations();
  if (activeConversationId !== null) await loadHistory(from, activeConversationId);
}

// P69 — the Warden tab group can change from `chrome.tabs.onRemoved` firing (a grouped tab
// closing) with no popup request in flight, so the panel needs its own broadcast to notice.
setGroupChangeListener(() => broadcast({ type: "groupChanged" }));

async function handleRequest(request: PopupRequest): Promise<unknown> {
  switch (request.type) {
    case "getStatus":
      return { status: currentStatus, history, savedSettings: await getSavedSettings(), approvals, ...conversationState() };

    case "connect": {
      connection?.goodbye();
      const deviceId = await getOrCreateDeviceId();
      const hub = `${request.host}:${request.port}`;
      setStatus({ kind: "connecting" });
      let next: ServerConnection;
      try {
        next = await ServerConnection.connect({
          host: request.host,
          port: request.port,
          secure: request.secure,
          deviceId,
          deviceName: request.deviceName,
          authKey: request.authKey,
          deviceToken: (await getDeviceTokens())[hub],
          toolSpecs,
          toolHandlers,
        });
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setStatus({ kind: "failure", message });
        return { ok: false, error: message };
      }
      if (next.issuedDeviceToken !== undefined) await saveDeviceToken(hub, next.issuedDeviceToken);
      connection = next;
      history = [];
      conversations = [];
      pendingTurns = {};
      agentIds = [];
      setApprovals([]);
      if (activeConversationId === null) {
        const stored = await chrome.storage.local.get(STORAGE_KEY_ACTIVE_CONVERSATION);
        activeConversationId = (stored[STORAGE_KEY_ACTIVE_CONVERSATION] as string | undefined) ?? null;
      }
      connection.onStatusChange((status) => {
        // A dropped connection never hears the answers it was waiting on, nor its approvals.
        if (status.kind !== "connected") {
          pendingTurns = {};
          setApprovals([]);
        }
        setStatus(status);
      });
      connection.onApproval((event) => {
        if (event.kind === "prompt") setApprovals([...approvals, event.prompt]);
        else setApprovals(approvals.filter((a) => a.approvalId !== event.approvalId));
      });
      connection.onConversationChanged((conversationId) => {
        // An agent left a note here, or answered one: new list, and the transcript if it's open.
        void refreshConversations(next);
        if (conversationId === activeConversationId && !(conversationId in pendingTurns)) void loadHistory(next, conversationId);
      });
      connection.onChatMessage((entry, conversationId) => {
        const id = conversationId ?? activeConversationId;
        if (id !== null) delete pendingTurns[id];
        if (id === activeConversationId) addChatEntry(entry);
        broadcastConversations();
        // New title/order — and a conversation started here now exists on the hub.
        void refreshConversations(next);
      });
      setStatus(connection.status);
      await chrome.storage.local.set({
        [STORAGE_KEY_SETTINGS]: {
          host: request.host,
          port: request.port,
          deviceName: request.deviceName,
          authKey: request.authKey,
          secure: request.secure,
        } satisfies ConnectionSettings,
      });
      void restoreConversations(next);
      void refreshAgents(next);
      return { ok: true };
    }

    case "disconnect":
      connection?.goodbye();
      connection = null;
      setApprovals([]);
      setStatus({ kind: "disconnected" });
      return { ok: true };

    case "sendChat": {
      if (!connection || connection.status.kind !== "connected") {
        return { ok: false, error: "not connected" };
      }
      if (activeConversationId === null) setActiveConversation(crypto.randomUUID());
      const id = activeConversationId as string;
      addChatEntry({ role: "user", content: request.message });
      pendingTurns[id] = request.message;
      // The folder only counts on the turn that starts the conversation: the hub keeps the one it was made with.
      const startsHere = !conversations.some((c) => c.id === id);
      if (startsHere) {
        // Shown until the hub's list has it — same title the hub gives it (`title_from`).
        const now = Date.now();
        const collapsed = request.message.split(/\s+/).filter(Boolean).join(" ");
        const title = [...collapsed].length > 40 ? `${[...collapsed].slice(0, 40).join("")}…` : collapsed;
        conversations = [{ id, title, createdAt: now, updatedAt: now, ...(agentId !== null && { agentId }), ...(workdir !== null && { workdir }) }, ...conversations];
      }
      broadcastConversations();
      connection.sendChat(request.message, id, agentId ?? undefined, startsHere ? (workdir ?? undefined) : undefined);
      return { ok: true };
    }

    case "selectWorkdir": {
      // Chosen before the first message and never after: a conversation on the hub keeps its own folder.
      if (activeConversationId !== null && conversations.some((c) => c.id === activeConversationId)) {
        return { ok: false, error: "a pasta é escolhida antes da primeira mensagem e não muda depois" };
      }
      workdir = request.path;
      broadcastConversations();
      return { ok: true };
    }

    case "listDirs": {
      if (!connection || connection.status.kind !== "connected") return { ok: false, error: "not connected" };
      try {
        return { ok: true, listing: await connection.listDirs(request.path) };
      } catch (err) {
        return { ok: false, error: err instanceof Error ? err.message : String(err) };
      }
    }

    case "selectAgent":
      agentId = request.agentId;
      broadcastConversations();
      return { ok: true };

    case "refreshAgents":
      if (connection) await refreshAgents(connection);
      return { ok: true };

    case "resolveApproval":
      setApprovals(approvals.filter((a) => a.approvalId !== request.approvalId));
      connection?.resolveApproval(request.approvalId, request.approved);
      return { ok: true };

    case "selectConversation":
      if (request.conversationId !== activeConversationId) openConversation(request.conversationId);
      return { ok: true };

    case "newConversation":
      // Already on an empty, never-sent conversation — nothing to leave behind.
      if (activeConversationId === null || conversations.some((c) => c.id === activeConversationId) || history.length > 0) {
        openConversation(crypto.randomUUID());
      }
      return { ok: true };

    case "renameConversation":
    case "deleteConversation": {
      const from = connection;
      if (!from || from.status.kind !== "connected") return { ok: false, error: "not connected" };
      try {
        if (request.type === "renameConversation") await from.renameConversation(request.conversationId, request.title);
        else await from.deleteConversation(request.conversationId);
      } catch (err) {
        return { ok: false, error: err instanceof Error ? err.message : String(err) };
      }
      const list = await refreshConversations(from);
      if (request.type === "deleteConversation" && request.conversationId === activeConversationId) {
        openConversation(list?.[0]?.id ?? crypto.randomUUID());
      }
      return { ok: true };
    }

    case "listAgentTasks":
    case "controlAgentTask":
      if (!connection || connection.status.kind !== "connected") return { ok: false, tasks: [], error: "not connected" };
      try {
        const tasks = request.type === "listAgentTasks" ? await connection.listAgentTasks() : await connection.controlAgentTask(request.pairingKey, request.taskId, request.action);
        return { ok: true, tasks };
      } catch (err) {
        return { ok: false, tasks: [], error: err instanceof Error ? err.message : String(err), authRejected: err instanceof HubRequestError && err.authRejected };
      }

    case "listHubAgents":
    case "editAgentOrg":
      if (!connection || connection.status.kind !== "connected") return { ok: false, error: "not connected" };
      try {
        const hub = request.type === "listHubAgents" ? await connection.listHubAgents() : await connection.editAgentOrg(request.pairingKey, request.edit);
        // An edit can rename nobody, but it can add or remove agents: the chat's agent selector follows.
        if (request.type === "editAgentOrg") void refreshAgents(connection);
        return { ok: true, hub };
      } catch (err) {
        return { ok: false, error: err instanceof Error ? err.message : String(err), authRejected: err instanceof HubRequestError && err.authRejected };
      }

    case "listSkills":
      if (!connection || connection.status.kind !== "connected") return { ok: false, skills: [], error: "not connected" };
      try {
        return { ok: true, skills: await connection.listSkills() };
      } catch (err) {
        return { ok: false, skills: [], error: err instanceof Error ? err.message : String(err) };
      }

    case "saveSkill":
    case "deleteSkill":
      if (!connection || connection.status.kind !== "connected") return { ok: false, error: "not connected" };
      try {
        if (request.type === "saveSkill") await connection.saveSkill(request.skill, request.overwrite);
        else await connection.deleteSkill(request.name);
        return { ok: true };
      } catch (err) {
        return { ok: false, error: err instanceof Error ? err.message : String(err) };
      }

    case "discoverHubs":
      if (!supportsHubDiscovery()) {
        return { ok: false, hubs: [], error: "LAN discovery isn't available in this browser — type the hub's address instead" };
      }
      try {
        return { ok: true, hubs: await discoverHubs(request.port) };
      } catch (err) {
        return { ok: false, hubs: [], error: err instanceof Error ? err.message : String(err) };
      }

    case "addTabToGroup":
      try {
        return { ok: true, tab: await addActiveTabToGroup() };
      } catch (err) {
        return { ok: false, error: err instanceof Error ? err.message : String(err) };
      }

    case "removeTabFromGroup":
      await removeTabFromGroup(request.tabId);
      return { ok: true };

    case "listGroupTabs":
      try {
        return { ok: true, tabs: await listGroupTabs() };
      } catch (err) {
        return { ok: false, tabs: [], error: err instanceof Error ? err.message : String(err) };
      }
  }
}

chrome.runtime.onMessage.addListener((request: PopupRequest, _sender, sendResponse) => {
  handleRequest(request)
    .then(sendResponse)
    .catch((err: unknown) => sendResponse({ ok: false, error: err instanceof Error ? err.message : String(err) }));
  return true; // keep the message channel open for the async response above
});
