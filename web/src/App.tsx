import { useCallback, useEffect, useRef, useState } from "react";
import "./App.css";
import ChatView from "./components/ChatView";
import LoginView from "./components/LoginView";
import SkillsView from "./components/SkillsView";
import { HandshakeError, historyToEntries, hubUrl, ServerConnection, type ChatEntry } from "./hub/connection";
import { loadIdentity, saveIdentity, type Identity } from "./hub/identity";

/** How much of the conversation to load on (re)connect — same cap the extension uses. */
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

export default function App() {
  const identityRef = useRef<Identity>(loadIdentity());
  const connRef = useRef<ServerConnection | null>(null);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const reconnectAttempt = useRef(0);

  const [phase, setPhase] = useState<Phase>(identityRef.current.deviceToken ? { kind: "connecting" } : { kind: "login" });
  const [serverName, setServerName] = useState<string | null>(null);
  const [entries, setEntries] = useState<ChatEntry[]>([]);
  const [pending, setPendingState] = useState(false);
  /** Mirrors `pending` for the connection's callbacks, which outlive any one render. */
  const pendingRef = useRef(false);
  const setPending = useCallback((value: boolean) => {
    pendingRef.current = value;
    setPendingState(value);
  }, []);
  const [view, setView] = useState<View>("chat");
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
        const message = err instanceof Error ? err.message : String(err);
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

      connection.onChatMessage((entry) => {
        setEntries((current) => [...current, entry]);
        setPending(false);
      });
      connection.onStatusChange((status) => {
        if (status.kind === "connected" || connRef.current !== connection) return;
        connRef.current = null;
        setConn(null);
        if (pendingRef.current) {
          setEntries((current) => [...current, { role: "error", content: "A conexão caiu antes da resposta chegar.", attachments: [] }]);
          setPending(false);
        }
        if (status.kind === "disconnected") return; // our own goodbye (logout)
        if (connection.wasRejected) {
          forgetToken();
          setPhase({ kind: "login", error: status.kind === "failure" ? status.message : undefined });
          return;
        }
        setPhase({ kind: "ready", connected: false });
        scheduleReconnect();
      });

      try {
        const history = await connection.fetchHistory(HISTORY_LIMIT);
        if (connRef.current === connection) setEntries(historyToEntries(history));
      } catch (err) {
        if (connRef.current === connection) {
          setEntries([{ role: "error", content: `Não foi possível carregar o histórico: ${err instanceof Error ? err.message : String(err)}`, attachments: [] }]);
        }
      }
    },
    // `scheduleReconnect` (below) only touches refs and state setters, so it's safe to leave out.
    [clearReconnect, forgetToken, setPending],
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
    setPending(false);
    setServerName(null);
    setPhase({ kind: "login" });
  }

  function handleSend(message: string) {
    const connection = connRef.current;
    if (!connection) return;
    setEntries((current) => [...current, { role: "user", content: message, attachments: [] }]);
    setPending(true);
    try {
      connection.sendChat(message);
    } catch (err) {
      setPending(false);
      setEntries((current) => [...current, { role: "error", content: err instanceof Error ? err.message : String(err), attachments: [] }]);
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
        {view === "chat" ? <ChatView entries={entries} pending={pending} disabled={!phase.connected} onSend={handleSend} /> : <SkillsView conn={conn} />}
      </main>
    </div>
  );
}
