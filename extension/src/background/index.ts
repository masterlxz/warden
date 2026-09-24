/**
 * Background service worker (Fase 8.1/8.2) — owns the single `ServerConnection` instance and
 * this session's chat history. Lives here (never in the side panel, which the user can close
 * independently) so the connection survives the panel opening/closing repeatedly while the user
 * chats.
 *
 * `history` is in-memory only, not persisted to `chrome.storage` — the hub already keeps this
 * device's conversation (P40), so every connect reloads it from there (`loadHistory`); a service
 * worker that gets evicted loses the connection and the transcript together, and the next connect
 * brings the transcript back. No automatic
 * reconnect either: a fresh service worker reports `disconnected`, and the panel shows the
 * connection form again — same accepted gap `server_connection.dart` (Fase 7.2) drew, see its doc
 * comment.
 */

import { ServerConnection, type ChatEntry, type ConnectionStatus } from "./connection";
import { discoverHubs } from "./discovery";
import type { ConnectionSettings, PopupRequest } from "./popup_protocol";
import { toolSpecs, toolHandlers } from "./tools";
import { addActiveTabToGroup, listGroupTabs, removeTabFromGroup, setGroupChangeListener } from "./tab_group";
import { setUpPanelOpening, supportsHubDiscovery } from "./platform";

// Makes clicking the toolbar icon open the chat panel (Chrome's side panel, Firefox's sidebar)
// instead of requiring a `default_popup`. Without this call the icon click has no effect.
setUpPanelOpening();

const STORAGE_KEY_DEVICE_ID = "deviceId";
const STORAGE_KEY_SETTINGS = "connectionSettings";
const STORAGE_KEY_DEVICE_TOKENS = "deviceTokens";

let connection: ServerConnection | null = null;
let history: ChatEntry[] = [];
let currentStatus: ConnectionStatus = { kind: "disconnected" };

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

/** How many past messages to show on connect — same cut as the mobile app (P40). */
const HISTORY_LIMIT = 100;

/** P40 — puts the hub's persisted conversation in front of whatever was already said on this
 * connection (a message sent before the reply lands stays after it, where it belongs). Runs after
 * `connect` returns, so a slow history never delays the panel showing the chat. */
async function loadHistory(from: ServerConnection): Promise<void> {
  let loaded: ChatEntry[];
  try {
    loaded = (await from.fetchHistory(HISTORY_LIMIT)).map((m) => ({ role: m.role, content: m.content }));
  } catch (err) {
    loaded = [{ role: "error", content: `Could not load earlier messages: ${err instanceof Error ? err.message : String(err)}` }];
  }
  if (connection !== from) return; // disconnected or reconnected elsewhere meanwhile
  history = [...loaded, ...history];
  broadcast({ type: "historyLoaded", history });
}

// P69 — the Warden tab group can change from `chrome.tabs.onRemoved` firing (a grouped tab
// closing) with no popup request in flight, so the panel needs its own broadcast to notice.
setGroupChangeListener(() => broadcast({ type: "groupChanged" }));

async function handleRequest(request: PopupRequest): Promise<unknown> {
  switch (request.type) {
    case "getStatus":
      return { status: currentStatus, history, savedSettings: await getSavedSettings() };

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
      connection.onStatusChange(setStatus);
      connection.onChatMessage(addChatEntry);
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
      void loadHistory(next);
      return { ok: true };
    }

    case "disconnect":
      connection?.goodbye();
      connection = null;
      setStatus({ kind: "disconnected" });
      return { ok: true };

    case "sendChat":
      if (!connection || connection.status.kind !== "connected") {
        return { ok: false, error: "not connected" };
      }
      addChatEntry({ role: "user", content: request.message });
      connection.sendChat(request.message);
      return { ok: true };

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
