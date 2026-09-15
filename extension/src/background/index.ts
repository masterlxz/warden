/**
 * Background service worker (Fase 8.1/8.2) — owns the single `ServerConnection` instance and
 * this session's chat history. Lives here (never in the popup, which MV3 destroys on close) so
 * the connection survives the popup opening/closing repeatedly while the user chats.
 *
 * `history` is in-memory only, not persisted to `chrome.storage` — if this service worker gets
 * evicted, the connection dies right along with it (same underlying WebSocket), so losing the
 * transcript at the same moment isn't a separate failure mode to guard against. No automatic
 * reconnect either: a fresh service worker reports `disconnected`, and the popup shows the
 * connection form again — same accepted gap `server_connection.dart` (Fase 7.2) drew, see its doc
 * comment.
 */

import { ServerConnection, type ChatEntry, type ConnectionStatus } from "./connection";
import type { ConnectionSettings, PopupRequest } from "./popup_protocol";

const STORAGE_KEY_DEVICE_ID = "deviceId";
const STORAGE_KEY_SETTINGS = "connectionSettings";

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

async function getSavedSettings(): Promise<Partial<ConnectionSettings>> {
  const stored = await chrome.storage.local.get(STORAGE_KEY_SETTINGS);
  return (stored[STORAGE_KEY_SETTINGS] as ConnectionSettings | undefined) ?? {};
}

/** Best-effort broadcast to any popup currently open and listening — a closed popup means no
 * receiver, which `sendMessage` reports as a rejected promise; that's the expected common case,
 * not an error worth surfacing. */
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

async function handleRequest(request: PopupRequest): Promise<unknown> {
  switch (request.type) {
    case "getStatus":
      return { status: currentStatus, history, savedSettings: await getSavedSettings() };

    case "connect": {
      connection?.goodbye();
      const deviceId = await getOrCreateDeviceId();
      setStatus({ kind: "connecting" });
      let next: ServerConnection;
      try {
        next = await ServerConnection.connect({
          host: request.host,
          port: request.port,
          deviceId,
          deviceName: request.deviceName,
          authKey: request.authKey,
        });
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setStatus({ kind: "failure", message });
        return { ok: false, error: message };
      }
      connection = next;
      history = [];
      connection.onStatusChange(setStatus);
      connection.onChatMessage(addChatEntry);
      setStatus(connection.status);
      await chrome.storage.local.set({
        [STORAGE_KEY_SETTINGS]: { host: request.host, port: request.port, deviceName: request.deviceName, authKey: request.authKey },
      });
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
  }
}

chrome.runtime.onMessage.addListener((request: PopupRequest, _sender, sendResponse) => {
  handleRequest(request)
    .then(sendResponse)
    .catch((err: unknown) => sendResponse({ ok: false, error: err instanceof Error ? err.message : String(err) }));
  return true; // keep the message channel open for the async response above
});
