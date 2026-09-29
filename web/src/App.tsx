import { useCallback, useEffect, useRef, useState } from "react";
import "./App.css";
import ApprovalModal from "./components/ApprovalModal";
import ChangePasswordView from "./components/ChangePasswordView";
import ChatView from "./components/ChatView";
import ConversationList from "./components/ConversationList";
import DevicesView from "./components/DevicesView";
import ApiKeysSection from "./components/ApiKeysSection";
import LoginView, { type LoginCredentials } from "./components/LoginView";
import MyAgentsView from "./components/MyAgentsView";
import PeopleView from "./components/PeopleView";
import RecoveryCodeView from "./components/RecoveryCodeView";
import SettingsView from "./components/SettingsView";
import SkillsView from "./components/SkillsView";
import SyncView from "./components/SyncView";
import TasksView from "./components/TasksView";
import UsageView from "./components/UsageView";
import VaultView from "./components/VaultView";
import { HandshakeError, historyToEntries, hubUrl, ServerConnection, type ApprovalPrompt, type ChatEntry } from "./hub/connection";
import { loadIdentity, loadLastConversation, newConversationId, saveIdentity, saveLastConversation, type Identity } from "./hub/identity";
import type { Attachment, ConversationSummary, UserInfo } from "./hub/messages";

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

type View = "chat" | "vault" | "usage" | "skills" | "tasks" | "devices" | "people" | "sync" | "settings" | "myAgents" | "myApi";

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** The title the hub gives a new conversation (`title_from` in `warden-bootstrap`), shown in the
 * list until the hub's own list comes back. */
function titleFrom(message: string): string {
  const collapsed = message.split(/\s+/).filter(Boolean).join(" ");
  return [...collapsed].length > 40 ? `${[...collapsed].slice(0, 40).join("")}…` : collapsed;
}

/** What the hub titles a turn with no words after (`title_seed` in `chat_input.rs`). */
function titleSeed(message: string, attachments: Attachment[]): string {
  if (message.trim() !== "" || attachments.length === 0) return message;
  return attachments.every((a) => a.mimeType.startsWith("image/")) ? "Image" : "Document";
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
  const [pendingTurns, setPendingTurnsState] = useState<Record<string, ChatEntry>>({});
  const pendingTurnsRef = useRef<Record<string, ChatEntry>>({});
  const setPendingTurns = useCallback((update: (current: Record<string, ChatEntry>) => Record<string, ChatEntry>) => {
    pendingTurnsRef.current = update(pendingTurnsRef.current);
    setPendingTurnsState(pendingTurnsRef.current);
  }, []);
  const [view, setView] = useState<View>("chat");
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [conn, setConn] = useState<ServerConnection | null>(null);
  /** The hub's configured agents (P46), for the chat's agent selector. */
  const [agentIds, setAgentIds] = useState<string[]>([]);
  /** The agent the open conversation speaks with — "" for none. */
  const [agentId, setAgentId] = useState("");
  /** Tools in this browser's turns waiting for a yes (P46), oldest first. */
  const [approvals, setApprovals] = useState<ApprovalPrompt[]>([]);
  /** P84: the member this browser belongs to — `undefined` for the owner. */
  const [user, setUser] = useState<UserInfo | undefined>(undefined);
  /** A member asked to change their password (the provisional one forces it without asking). */
  const [changingPassword, setChangingPassword] = useState(false);
  /** P84 fatia 4: a recovery code of the member's encrypted data, waiting to be shown once. `replacing`: it
   * takes the place of an earlier one. */
  const [recoveryCode, setRecoveryCode] = useState<{ code: string; replacing: boolean } | null>(null);
  /** Mirrors `conversations` for the connection's callbacks. */
  const conversationsRef = useRef<ConversationSummary[]>([]);
  conversationsRef.current = conversations;

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

  /** Re-reads the agent list. Without it (settings unreadable) the selector just offers none. */
  const refreshAgents = useCallback(async (connection: ServerConnection) => {
    try {
      const { settings } = await connection.requestSettings();
      if (connRef.current === connection) setAgentIds(settings.agents.map((a) => a.id));
    } catch {
      if (connRef.current === connection) setAgentIds([]);
    }
  }, []);

  /** Shows `id`'s transcript, fetched from the hub — plus its unanswered turn, which the hub only
   * saves once the answer arrives. */
  const loadConversation = useCallback(async (connection: ServerConnection, id: string) => {
    const withPending = (loaded: ChatEntry[]): ChatEntry[] => {
      const waiting = pendingTurnsRef.current[id];
      return waiting === undefined ? loaded : [...loaded, waiting];
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

  /** Connects with the stored token, or pairs with `credentials` when given — the pairing key, or
   * a member's username and password (P84). */
  const connect = useCallback(
    async (credentials?: LoginCredentials) => {
      clearReconnect();
      const identity = identityRef.current;
      const usingToken = credentials === undefined;
      let connection: ServerConnection;
      try {
        connection = await ServerConnection.connect({
          url: hubUrl(),
          deviceId: identity.deviceId,
          deviceName: identity.deviceName,
          authKey: credentials?.kind === "key" ? credentials.authKey : "",
          ...(credentials?.kind === "user" && { username: credentials.username, password: credentials.password }),
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
      setUser(connection.user);
      setPhase({ kind: "ready", connected: true });

      connection.onChatMessage((entry, conversationId) => {
        const id = conversationId ?? activeIdRef.current;
        setPendingTurns(({ [id]: _answered, ...rest }) => rest);
        if (id === activeIdRef.current) setEntries((current) => [...current, entry]);
        // New title/order — and a conversation started here now exists on the hub.
        void refreshConversations(connection);
      });
      connection.onApproval((event) => {
        if (event.kind === "prompt") setApprovals((queue) => [...queue, event.prompt]);
        else setApprovals((queue) => queue.filter((p) => p.approvalId !== event.approvalId));
      });
      // Signing in turned encryption on for this member's data (P84 fatia 4): the code is shown once.
      connection.onRecoveryCode((code) => {
        setRecoveryCode({ code, replacing: false });
        setUser((current) => (current ? { ...current, encrypted: true } : current));
      });
      // An agent left a message for another, or answered one (P46): new or changed conversations.
      connection.onConversationsChanged((conversationId) => {
        void refreshConversations(connection);
        if (conversationId === activeIdRef.current && !(conversationId in pendingTurnsRef.current)) {
          void loadConversation(connection, conversationId);
        }
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
        // Nobody can answer those any more: the hub counts them as a no.
        setApprovals([]);
        if (status.kind === "disconnected") return; // our own goodbye (logout)
        if (connection.wasRejected) {
          forgetToken();
          setPhase({ kind: "login", error: status.kind === "failure" ? status.message : undefined });
          return;
        }
        setPhase({ kind: "ready", connected: false });
        scheduleReconnect();
      });

      void refreshAgents(connection);
      const list = await refreshConversations(connection);
      if (connRef.current !== connection) return;
      // Keep the open conversation across reconnects. A remembered one that was deleted
      // elsewhere gives way to the most recent; with none at all, a fresh one waits for its first message.
      const current = activeIdRef.current;
      if (list && !list.some((c) => c.id === current) && !(current in pendingTurnsRef.current)) {
        setActiveId(list[0]?.id ?? newConversationId());
      }
      const open = list?.find((c) => c.id === activeIdRef.current);
      if (open) setAgentId(open.agentId ?? "");
      await loadConversation(connection, activeIdRef.current);
    },
    // `scheduleReconnect` (below) only touches refs and state setters, so it's safe to leave out.
    [clearReconnect, forgetToken, loadConversation, refreshAgents, refreshConversations, setActiveId, setPendingTurns],
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

  function handleLogin(credentials: LoginCredentials, deviceName: string) {
    identityRef.current = { ...identityRef.current, deviceName };
    saveIdentity(identityRef.current);
    setPhase({ kind: "connecting" });
    void connect(credentials);
  }

  /** The member's own password is in: what the provisional one kept closed can load now. */
  function handlePasswordChanged(newRecoveryCode?: string) {
    // The change opened their data key (or made it), so it's no longer shut either.
    setUser((current) => (current ? { ...current, mustChangePassword: false, needsRecovery: false, locked: false, encrypted: current.encrypted || newRecoveryCode !== undefined } : current));
    if (newRecoveryCode !== undefined) setRecoveryCode({ code: newRecoveryCode, replacing: false });
    setChangingPassword(false);
    const connection = connRef.current;
    if (!connection) return;
    void refreshAgents(connection);
    void refreshConversations(connection).then(() => loadConversation(connection, activeIdRef.current));
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
    setApprovals([]);
    setAgentIds([]);
    setServerName(null);
    setUser(undefined);
    setChangingPassword(false);
    setRecoveryCode(null);
    setView("chat");
    setPhase({ kind: "login" });
  }

  function handleApproval(approvalId: number, approved: boolean) {
    setApprovals((queue) => queue.filter((p) => p.approvalId !== approvalId));
    try {
      connRef.current?.resolveApproval(approvalId, approved);
    } catch {
      // The connection is gone; the hub already counts an unanswered request as a no.
    }
  }

  function handleSend(message: string, attachments: Attachment[]) {
    const connection = connRef.current;
    if (!connection) return;
    const id = activeIdRef.current;
    const entry: ChatEntry = { role: "user", content: message, attachments };
    setEntries((current) => [...current, entry]);
    setPendingTurns((current) => ({ ...current, [id]: entry }));
    if (!conversations.some((c) => c.id === id)) {
      const now = Date.now();
      setConversations((current) => [
        { id, title: titleFrom(titleSeed(message, attachments)), createdAt: now, updatedAt: now, ...(agentId && { agentId }) },
        ...current,
      ]);
    }
    try {
      connection.sendChat(message, id, attachments, agentId || undefined);
    } catch (err) {
      setPendingTurns(({ [id]: _failed, ...rest }) => rest);
      setEntries((current) => [...current, { role: "error", content: errorText(err), attachments: [] }]);
    }
  }

  async function handleTranscribe(audio: Attachment): Promise<string> {
    const connection = connRef.current;
    if (!connection) throw new Error("sem conexão com o hub");
    return connection.transcribe(audio);
  }

  async function handleExtendLimit(limitId: string): Promise<void> {
    const connection = connRef.current;
    if (!connection) throw new Error("sem conexão com o hub");
    await connection.extendLimit(limitId);
  }

  function openConversation(id: string) {
    setSidebarOpen(false);
    setView("chat");
    if (id === activeIdRef.current) return;
    setActiveId(id);
    setEntries([]);
    // Each conversation remembers the agent it spoke with last (P46).
    setAgentId(conversationsRef.current.find((c) => c.id === id)?.agentId ?? "");
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
    setAgentId("");
  }

  function showView(next: View) {
    setView(next);
    // Agents may have been added or renamed in Settings meanwhile.
    if (next === "chat" && connRef.current) void refreshAgents(connRef.current);
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

  // The code comes before anything else: it's shown once, and the person has to say they kept it.
  if (conn && recoveryCode) {
    return <RecoveryCodeView code={recoveryCode.code} replacing={recoveryCode.replacing} onDone={() => setRecoveryCode(null)} />;
  }

  if (conn && user && (user.mustChangePassword || changingPassword)) {
    return (
      <ChangePasswordView
        conn={conn}
        name={user.name}
        required={user.mustChangePassword}
        needsRecovery={user.needsRecovery}
        encrypted={user.encrypted}
        onDone={handlePasswordChanged}
        onNewCode={(code) => {
          setRecoveryCode({ code, replacing: true });
          setChangingPassword(false);
        }}
        onCancel={() => setChangingPassword(false)}
        onLogout={handleLogout}
      />
    );
  }

  const activeTitle = conversations.find((c) => c.id === activeId)?.title ?? "Nova conversa";
  /** P84: a member sees their chat, vault and skills — the rest is the owner's. */
  const isOwner = user === undefined;

  return (
    <div className="app">
      <header className="app-header">
        <div className="app-brand">
          <img src="/favicon.svg" alt="" width={24} height={24} />
          <span className="app-hub-name">{serverName ?? "Warden"}</span>
          <span className={`status-dot ${phase.connected ? "status-dot--on" : "status-dot--off"}`} title={phase.connected ? "Conectado" : "Reconectando…"} />
        </div>
        <nav className="app-tabs" aria-label="Seções">
          <button type="button" className={view === "chat" ? "tab tab--active" : "tab"} onClick={() => showView("chat")}>
            Chat
          </button>
          <button type="button" className={view === "vault" ? "tab tab--active" : "tab"} onClick={() => setView("vault")}>
            Vault
          </button>
          {isOwner && (
            <button type="button" className={view === "usage" ? "tab tab--active" : "tab"} onClick={() => setView("usage")}>
              Uso
            </button>
          )}
          <button type="button" className={view === "skills" ? "tab tab--active" : "tab"} onClick={() => setView("skills")}>
            Skills
          </button>
          {isOwner && (
            <>
              <button type="button" className={view === "tasks" ? "tab tab--active" : "tab"} onClick={() => setView("tasks")}>
                Tarefas
              </button>
              <button type="button" className={view === "devices" ? "tab tab--active" : "tab"} onClick={() => setView("devices")}>
                Aparelhos
              </button>
              <button type="button" className={view === "people" ? "tab tab--active" : "tab"} onClick={() => setView("people")}>
                Pessoas
              </button>
              <button type="button" className={view === "sync" ? "tab tab--active" : "tab"} onClick={() => setView("sync")}>
                Sync
              </button>
              <button
                type="button"
                className={view === "settings" ? "tab tab--active" : "tab"}
                onClick={() => setView("settings")}
                aria-label="Configurações"
                title="Configurações"
              >
                ⚙
              </button>
            </>
          )}
          {!isOwner && (
            <>
              <button type="button" className={view === "myAgents" ? "tab tab--active" : "tab"} onClick={() => setView("myAgents")}>
                Agentes
              </button>
              <button type="button" className={view === "myApi" ? "tab tab--active" : "tab"} onClick={() => setView("myApi")}>
                API
              </button>
            </>
          )}
        </nav>
        {user && (
          <button type="button" className="link-button" onClick={() => setChangingPassword(true)} title="Trocar senha">
            {user.name}
          </button>
        )}
        <button type="button" className="link-button" onClick={handleLogout}>
          Sair
        </button>
      </header>

      {!phase.connected && <p className="banner">A conexão com o hub caiu. Reconectando…</p>}
      {user?.locked && (
        <p className="banner">Os seus dados estão trancados: o hub reiniciou e só abre a chave com a sua senha. Saia e entre de novo com a senha para abri-los.</p>
      )}

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
                {(agentIds.length > 0 || agentId !== "") && (
                  <label className="agent-picker">
                    <span className="agent-picker-label">Agente</span>
                    <select value={agentId} onChange={(e) => setAgentId(e.target.value)} disabled={activeId in pendingTurns}>
                      <option value="">Nenhum</option>
                      {agentIds.map((id) => (
                        <option key={id} value={id}>
                          {id}
                        </option>
                      ))}
                      {agentId !== "" && !agentIds.includes(agentId) && <option value={agentId}>{agentId} (removido)</option>}
                    </select>
                  </label>
                )}
              </div>
              <ChatView
                entries={entries}
                pending={activeId in pendingTurns}
                disabled={!phase.connected}
                onSend={handleSend}
                onTranscribe={handleTranscribe}
                onExtendLimit={handleExtendLimit}
              />
            </div>
          </div>
        ) : view === "vault" ? (
          <VaultView conn={conn} />
        ) : view === "usage" ? (
          <UsageView conn={conn} />
        ) : view === "tasks" ? (
          <TasksView conn={conn} onOpenConversation={openConversation} />
        ) : view === "devices" ? (
          <DevicesView conn={conn} />
        ) : view === "people" ? (
          <PeopleView conn={conn} />
        ) : view === "myAgents" ? (
          <MyAgentsView conn={conn} onChanged={() => connRef.current && void refreshAgents(connRef.current)} />
        ) : view === "myApi" ? (
          <div className="usage-view">
            <ApiKeysSection conn={conn} member />
          </div>
        ) : view === "sync" ? (
          <SyncView conn={conn} />
        ) : view === "settings" ? (
          <SettingsView conn={conn} />
        ) : (
          <SkillsView conn={conn} />
        )}
      </main>
      <ApprovalModal queue={approvals} onAnswer={handleApproval} />
    </div>
  );
}
