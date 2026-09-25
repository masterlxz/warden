import { useCallback, useEffect, useRef, useState } from "react";
import "./App.css";
import ChatView from "./components/ChatView";
import ConversationList from "./components/ConversationList";
import LoginView from "./components/LoginView";
import SkillsView from "./components/SkillsView";
import { HandshakeError, historyToEntries, hubUrl, ServerConnection, type ChatEntry } from "./hub/connection";
import { loadIdentity, loadLastConversation, newConversationId, saveIdentity, saveLastConversation, type Identity } from "./hub/identity";
import type { ConversationSummary } from "./hub/messages";

/** How much of a conversation to load when it's opened — same cap the extension uses. */
const HISTORY_LIMIT = 200;
/** Waits between reconnect attempts after the connection drops; the last one repeats. */
const RECONNECT_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 15_000, 30_000];

type Phase =
  /** Needs the pairing key (first visit, or the hub stopped accepting this browser's token). */
  | { kind: "login"; error?: string }
  /** First connection attempt of this visit, or a login in flight. */
  | { kind: "connecting" }
  /** Paired. `connected: false` = the connection dropped and a reconnect is scheduled. */
  | { kind: "ready"; connected: boolean };

type View = "chat" | "skills";

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** The title the hub gives a new conversation (`title_from` in `warden-bootstrap`), shown in the
 * list until the hub's own list comes back. */
function titleFrom(message: string): string {
  const collapsed = message.split(/\s+/).filter(Boolean).join(" ");
  return [...collapsed].length > 40 ? `${[...collapsed].slice(0, 40).join("")}…` : collapsed;
}

export default function App() {
  const identityRef = useRef<Identity>(loadIdentity());
  const connRef = useRef<ServerConnection | null>(null);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const reconnectAttempt = useRef(0);

  const [phase, setPhase] = useState<Phase>(identityRef.current.deviceToken ? { kind: "connecting" } : { kind: "login" });
  const [serverName, setServerName] = useState<string | null>(null);
  const [entries, setEntries] = useState<ChatEntry[]>([]);
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [conversationsError, setConversationsError] = useState<string | null>(null);
  const [activeId, setActiveIdState] = useState<string>(() => loadLastConversation() ?? newConversationId());
  /** Mirrors `activeId` for the connection's callbacks, which outlive any one render. */
  const activeIdRef = useRef(activeId);
  const setActiveId = useCallback((id: string) => {
    activeIdRef.current = id;
    setActiveIdState(id);
    saveLastConversation(id);
  }, []);
  /** Turns sent and not answered yet, by conversation id — each conversation waits on its own. */
  const [pendingTurns, setPendingTurnsState] = useState<Record<string, string>>({});
  const pendingTurnsRef = useRef<Record<string, string>>({});
  const setPendingTurns = useCallback((update: (current: Record<string, string>) => Record<string, string>) => {
    pendingTurnsRef.current = update(pendingTurnsRef.current);
    setPendingTurnsState(pendingTurnsRef.current);
  }, []);
  const [view, setView] = useState<View>("chat");
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [conn, setConn] = useState<ServerConnection | null>(null);

  const forgetToken = useCallback(() => {
    const { deviceToken: _dropped, ...rest } = identityRef.current;
    identityRef.current = rest;
    saveIdentity(rest);
  }, []);

  const clearReconnect = useCallback(() => {
    if (reconnectTimer.current !== null) {
      clearTimeout(reconnectTimer.current);
      reconnectTimer.current = null;
    }
  }, []);

  /** Re-reads the conversation list. Failures show above the list instead of in the chat. */
  const refreshConversations = useCallback(async (connection: ServerConnection) => {
    try {
      const list = await connection.listConversations();
      if (connRef.current !== connection) return;
      // A conversation started here whose first turn is still in flight isn't on the hub yet.
      setConversations((current) => [...current.filter((c) => c.id in pendingTurnsRef.current && !list.some((l) => l.id === c.id)), ...list]);
      setConversationsError(null);
      return list;
    } catch (err) {
      if (connRef.current === connection) setConversationsError(`Não foi possível carregar as conversas: ${errorText(err)}`);
      return undefined;
    }
  }, []);

  /** Shows `id`'s transcript, fetched from the hub — plus its unanswered turn, which the hub only
   * saves once the answer arrives. */
  const loadConversation = useCallback(async (connection: ServerConnection, id: string) => {
    const withPending = (loaded: ChatEntry[]): ChatEntry[] => {
      const waiting = pendingTurnsRef.current[id];
      return waiting === undefined ? loaded : [...loaded, { role: "user", content: waiting, attachments: [] }];
    };
    try {
      const history = await connection.fetchHistory(id, HISTORY_LIMIT);
      if (connRef.current === connection && activeIdRef.current === id) setEntries(withPending(historyToEntries(history)));
    } catch (err) {
      if (connRef.current === connection && activeIdRef.current === id) {
        setEntries(withPending([{ role: "error", content: `Não foi possível carregar o histórico: ${errorText(err)}`, attachments: [] }]));
      }
    }
  }, []);

  /** Connects with the stored token, or pairs with `authKey` when given. */
  const connect = useCallback(
    async (authKey?: string) => {
      clearReconnect();
      const identity = identityRef.current;
      const usingToken = authKey === undefined;
      let connection: ServerConnection;
      try {
        connection = await ServerConnection.connect({
          url: hubUrl(),
          deviceId: identity.deviceId,
          deviceName: identity.deviceName,
          authKey: authKey ?? "",
          ...(usingToken && identity.deviceToken !== undefined && { deviceToken: identity.deviceToken }),
        });
      } catch (err) {
        const message = errorText(err);
        if (err instanceof HandshakeError && err.authRejected) {
          // The hub doesn't know this token (revoked, or its registry was reset) — pair again.
          if (usingToken) forgetToken();
          setPhase({ kind: "login", error: usingToken ? `O hub não aceitou mais este navegador: ${message}` : message });
        } else if (usingToken) {
          scheduleReconnect();
        } else {
          setPhase({ kind: "login", error: message });
        }
        return;
      }

      if (connection.issuedDeviceToken) {
        identityRef.current = { ...identityRef.current, deviceToken: connection.issuedDeviceToken };
        saveIdentity(identityRef.current);
      }
      reconnectAttempt.current = 0;
      connRef.current = connection;
      setConn(connection);
      setServerName(connection.serverName);
      setPhase({ kind: "ready", connected: true });

      connection.onChatMessage((entry, conversationId) => {
        const id = conversationId ?? activeIdRef.current;
        setPendingTurns(({ [id]: _answered, ...rest }) => rest);
        if (id === activeIdRef.current) setEntries((current) => [...current, entry]);
        // New title/order — and a conversation started here now exists on the hub.
        void refreshConversations(connection);
      });
      connection.onStatusChange((status) => {
        if (status.kind === "connected" || connRef.current !== connection) return;
        connRef.current = null;
        setConn(null);
        if (activeIdRef.current in pendingTurnsRef.current) {
          setEntries((current) => [...current, { role: "error", content: "A conexão caiu antes da resposta chegar.", attachments: [] }]);
        }
        // The hub may still finish and save those turns, but this connection won't hear the answer.
        setPendingTurns(() => ({}));
        if (status.kind === "disconnected") return; // our own goodbye (logout)
        if (connection.wasRejected) {
          forgetToken();
          setPhase({ kind: "login", error: status.kind === "failure" ? status.message : undefined });
          return;
        }
        setPhase({ kind: "ready", connected: false });
        scheduleReconnect();
      });

      const list = await refreshConversations(connection);
      if (connRef.current !== connection) return;
      // Keep the open conversation across reconnects. A remembered one that was deleted
      // elsewhere gives way to the most recent; with none at all, a fresh one waits for its first message.
      const current = activeIdRef.current;
      if (list && !list.some((c) => c.id === current) && !(current in pendingTurnsRef.current)) {
        setActiveId(list[0]?.id ?? newConversationId());
      }
      await loadConversation(connection, activeIdRef.current);
    },
    // `scheduleReconnect` (below) only touches refs and state setters, so it's safe to leave out.
    [clearReconnect, forgetToken, loadConversation, refreshConversations, setActiveId, setPendingTurns],
  );

  function scheduleReconnect() {
    clearReconnect();
    const delay = RECONNECT_DELAYS_MS[Math.min(reconnectAttempt.current, RECONNECT_DELAYS_MS.length - 1)];
    reconnectAttempt.current += 1;
    setPhase((current) => (current.kind === "login" ? current : { kind: "ready", connected: false }));
    reconnectTimer.current = setTimeout(() => void connect(), delay);
  }

  useEffect(() => {
    if (identityRef.current.deviceToken) void connect();
    return () => {
      clearReconnect();
      connRef.current?.goodbye("page closed");
    };
  }, [connect, clearReconnect]);

  function handleLogin(authKey: string, deviceName: string) {
    identityRef.current = { ...identityRef.current, deviceName };
    saveIdentity(identityRef.current);
    setPhase({ kind: "connecting" });
    void connect(authKey);
  }

  function handleLogout() {
    clearReconnect();
    const connection = connRef.current;
    connRef.current = null;
    connection?.goodbye("signed out");
    forgetToken();
    setConn(null);
    setEntries([]);
    setConversations([]);
    setPendingTurns(() => ({}));
    setServerName(null);
    setPhase({ kind: "login" });
  }

  function handleSend(message: string) {
    const connection = connRef.current;
    if (!connection) return;
    const id = activeIdRef.current;
    setEntries((current) => [...current, { role: "user", content: message, attachments: [] }]);
    setPendingTurns((current) => ({ ...current, [id]: message }));
    if (!conversations.some((c) => c.id === id)) {
      const now = Date.now();
      setConversations((current) => [{ id, title: titleFrom(message), createdAt: now, updatedAt: now }, ...current]);
    }
    try {
      connection.sendChat(message, id);
    } catch (err) {
      setPendingTurns(({ [id]: _failed, ...rest }) => rest);
      setEntries((current) => [...current, { role: "error", content: errorText(err), attachments: [] }]);
    }
  }

  function openConversation(id: string) {
    setSidebarOpen(false);
    setView("chat");
    if (id === activeIdRef.current) return;
    setActiveId(id);
    setEntries([]);
    const connection = connRef.current;
    if (connection) void loadConversation(connection, id);
  }

  function handleNewConversation() {
    setSidebarOpen(false);
    setView("chat");
    // Already on an empty, never-sent conversation — nothing to leave behind.
    if (!conversations.some((c) => c.id === activeIdRef.current) && entries.length === 0) return;
    setActiveId(newConversationId());
    setEntries([]);
  }

  async function handleRename(id: string, title: string) {
    const connection = connRef.current;
    if (!connection) return;
    try {
      await connection.renameConversation(id, title);
      await refreshConversations(connection);
    } catch (err) {
      setConversationsError(`Não foi possível renomear: ${errorText(err)}`);
    }
  }

  async function handleDelete(id: string) {
    const connection = connRef.current;
    if (!connection) return;
    try {
      await connection.deleteConversation(id);
    } catch (err) {
      setConversationsError(`Não foi possível apagar: ${errorText(err)}`);
      return;
    }
    const list = await refreshConversations(connection);
    if (id !== activeIdRef.current) return;
    const next = list?.[0]?.id;
    if (next) {
      openConversation(next);
    } else {
      setActiveId(newConversationId());
      setEntries([]);
    }
  }

  if (phase.kind === "login" || phase.kind === "connecting") {
    return (
      <LoginView
        deviceName={identityRef.current.deviceName}
        hubAddress={window.location.host}
        busy={phase.kind === "connecting"}
        resuming={phase.kind === "connecting" && identityRef.current.deviceToken !== undefined}
        error={phase.kind === "login" ? phase.error : undefined}
        onSubmit={handleLogin}
      />
    );
  }

  const activeTitle = conversations.find((c) => c.id === activeId)?.title ?? "Nova conversa";

  return (
    <div className="app">
      <header className="app-header">
        <div className="app-brand">
          <img src="/favicon.svg" alt="" width={24} height={24} />
          <span className="app-hub-name">{serverName ?? "Warden"}</span>
          <span className={`status-dot ${phase.connected ? "status-dot--on" : "status-dot--off"}`} title={phase.connected ? "Conectado" : "Reconectando…"} />
        </div>
        <nav className="app-tabs" aria-label="Seções">
          <button type="button" className={view === "chat" ? "tab tab--active" : "tab"} onClick={() => setView("chat")}>
            Chat
          </button>
          <button type="button" className={view === "skills" ? "tab tab--active" : "tab"} onClick={() => setView("skills")}>
            Skills
          </button>
        </nav>
        <button type="button" className="link-button" onClick={handleLogout}>
          Sair
        </button>
      </header>

      {!phase.connected && <p className="banner">A conexão com o hub caiu. Reconectando…</p>}

      <main className="app-main">
        {view === "chat" ? (
          <div className={sidebarOpen ? "chat-layout chat-layout--drawer-open" : "chat-layout"}>
            <ConversationList
              conversations={conversations}
              activeId={activeId}
              pendingIds={Object.keys(pendingTurns)}
              error={conversationsError}
              disabled={!phase.connected}
              onOpen={openConversation}
              onNew={handleNewConversation}
              onRename={handleRename}
              onDelete={handleDelete}
            />
            <button type="button" className="drawer-scrim" aria-label="Fechar conversas" onClick={() => setSidebarOpen(false)} />
            <div className="chat-pane">
              <div className="chat-header">
                <button type="button" className="link-button drawer-toggle" onClick={() => setSidebarOpen(true)}>
                  ☰ Conversas
                </button>
                <span className="chat-title">{activeTitle}</span>
              </div>
              <ChatView entries={entries} pending={activeId in pendingTurns} disabled={!phase.connected} onSend={handleSend} />
            </div>
          </div>
        ) : (
          <SkillsView conn={conn} />
        )}
      </main>
    </div>
  );
}
