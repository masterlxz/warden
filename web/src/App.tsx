import { useCallback, useEffect, useRef, useState } from "react";
import "./App.css";
import ApprovalModal from "./components/ApprovalModal";
import ChangePasswordView from "./components/ChangePasswordView";
import ChatView from "./components/ChatView";
import ConversationList from "./components/ConversationList";
import DevicesView from "./components/DevicesView";
import FolderPicker from "./components/FolderPicker";
import ApiKeysSection from "./components/ApiKeysSection";
import LoginView, { type LoginCredentials, type TruthIdQr } from "./components/LoginView";
import MyAgentsView from "./components/MyAgentsView";
import PeopleView from "./components/PeopleView";
import ProjectsView from "./components/ProjectsView";
import RecoveryCodeView from "./components/RecoveryCodeView";
import RecoveryNoticeView from "./components/RecoveryNoticeView";
import SettingsView from "./components/SettingsView";
import OrganizationView from "./components/OrganizationView";
import AgentTasksView from "./components/AgentTasksView";
import SkillsView from "./components/SkillsView";
import SyncView from "./components/SyncView";
import TasksView from "./components/TasksView";
import WebhooksView from "./components/WebhooksView";
import UsageView from "./components/UsageView";
import VaultView from "./components/VaultView";
import { HandshakeError, historyToEntries, hubUrl, ServerConnection, type ApprovalPrompt, type ChatEntry } from "./hub/connection";
import { loadIdentity, loadLastConversation, newConversationId, saveIdentity, saveLastConversation, type Identity } from "./hub/identity";
import { applyEvent, type LiveTurn } from "./hub/liveTurn";
import { nextCodeMode } from "./hub/messages";
import type { Attachment, CodeMode, ConversationSummary, NodeInfo, ProjectDto, UserInfo } from "./hub/messages";
import { folderLabel, folderPlace, nodesWithFolders, parseNodeFolder } from "./hub/workdir";

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

type View = "chat" | "vault" | "usage" | "skills" | "projects" | "tasks" | "webhooks" | "devices" | "people" | "sync" | "settings" | "myAgents" | "myApi" | "organization" | "agentWork";

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
  /** P113: the TruthID sign-in waiting for the phone (its QR), and how to give up on it. */
  const [truthIdQr, setTruthIdQr] = useState<TruthIdQr | null>(null);
  const truthIdAbort = useRef<AbortController | null>(null);
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
  /** What a code engine has done so far in each running task (P103 b), by conversation id. */
  const [liveTurns, setLiveTurns] = useState<Record<string, LiveTurn>>({});
  const [view, setView] = useState<View>("chat");
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [conn, setConn] = useState<ServerConnection | null>(null);
  /** The hub's configured agents (P46), for the chat's agent selector. */
  const [agentIds, setAgentIds] = useState<string[]>([]);
  /** The agent the open conversation speaks with — "" for none. */
  const [agentId, setAgentId] = useState("");
  /** O agente cujas tarefas a aba "Trabalho dos agentes" mostra (definido a partir da árvore da organização). */
  const [agentWorkFilter, setAgentWorkFilter] = useState<string | null>(null);
  /** The person's projects (P103), for the chat's picker and the list's groups. */
  const [projects, setProjects] = useState<ProjectDto[]>([]);
  /** The project the open conversation is in — "" for none. Chosen before its first message, fixed after. */
  const [projectId, setProjectId] = useState("");
  /** The folder of the hub's machine the open conversation works in (P102) — "" for none. Chosen before its first
   * message, fixed after, and never together with a project. */
  const [workdir, setWorkdir] = useState("");
  const [pickingFolder, setPickingFolder] = useState(false);
  /** The nodes (P93), for the folder picker and for naming a folder on one. Only the owner can list them. */
  const [knownNodes, setKnownNodes] = useState<NodeInfo[]>([]);
  /** How much each code conversation asks (P103 b). Only here, never saved: a conversation opened again asks everything. */
  const [codeModes, setCodeModes] = useState<Record<string, CodeMode>>({});
  /** Tools in this browser's turns waiting for a yes (P46), oldest first. */
  const [approvals, setApprovals] = useState<ApprovalPrompt[]>([]);
  /** P84: the member this browser belongs to — `undefined` for the owner. */
  const [user, setUser] = useState<UserInfo | undefined>(undefined);
  /** A member asked to change their password (the provisional one forces it without asking). */
  const [changingPassword, setChangingPassword] = useState(false);
  /** P84 fatia 4: a recovery code of the member's encrypted data, waiting to be shown once. `replacing`: it
   * takes the place of an earlier one. */
  const [recoveryCode, setRecoveryCode] = useState<{ code: string; replacing: boolean } | null>(null);
  /** P84 fatia 4 parte B: the person put off the recovery notice (a policy change or a recovery by the owner) this session. */
  const [noticeLater, setNoticeLater] = useState(false);
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

  /** Re-reads the projects. Without them (a hub from before projects, a provisional password) there are none to pick. */
  const refreshProjects = useCallback(async (connection: ServerConnection) => {
    try {
      const list = await connection.listProjects();
      if (connRef.current === connection) setProjects(list);
    } catch {
      if (connRef.current === connection) setProjects([]);
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
      // A TruthID sign-in can be given up on (or asked again) while it waits for the phone.
      const abort = new AbortController();
      if (credentials?.kind === "truthid") {
        truthIdAbort.current?.abort();
        truthIdAbort.current = abort;
      }
      try {
        connection = await ServerConnection.connect({
          ...(credentials?.kind === "truthid" && { truthidLogin: true, signal: abort.signal, onTruthIdChallenge: (payload: string, expiresAtMs: number) => setTruthIdQr({ payload, expiresAtMs }) }),
          url: hubUrl(),
          deviceId: identity.deviceId,
          deviceName: identity.deviceName,
          authKey: credentials?.kind === "key" ? credentials.authKey : "",
          ...(credentials?.kind === "user" && { username: credentials.username, password: credentials.password }),
          ...(usingToken && identity.deviceToken !== undefined && { deviceToken: identity.deviceToken }),
        });
      } catch (err) {
        const message = errorText(err);
        // Given up on, or replaced by a fresh QR: whoever did it already decided what shows next.
        if (abort.signal.aborted) return;
        setTruthIdQr(null);
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

      setTruthIdQr(null);
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
        setLiveTurns(({ [id]: _done, ...rest }) => rest);
        if (id === activeIdRef.current) setEntries((current) => [...current, entry]);
        // New title/order — and a conversation started here now exists on the hub.
        void refreshConversations(connection);
      });
      // A code project's task, as the engine works on it (P103 b).
      connection.onChatEvent((event, id) => setLiveTurns((current) => ({ ...current, [id]: applyEvent(current[id], event) })));
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
        setLiveTurns({});
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
      void refreshProjects(connection);
      const list = await refreshConversations(connection);
      if (connRef.current !== connection) return;
      // Keep the open conversation across reconnects. A remembered one that was deleted
      // elsewhere gives way to the most recent; with none at all, a fresh one waits for its first message.
      const current = activeIdRef.current;
      if (list && !list.some((c) => c.id === current) && !(current in pendingTurnsRef.current)) {
        setActiveId(list[0]?.id ?? newConversationId());
      }
      const open = list?.find((c) => c.id === activeIdRef.current);
      if (open) {
        setAgentId(open.agentId ?? "");
        setProjectId(open.projectId ?? "");
        setWorkdir(open.workdir ?? "");
      }
      await loadConversation(connection, activeIdRef.current);
    },
    // `scheduleReconnect` (below) only touches refs and state setters, so it's safe to leave out.
    [clearReconnect, forgetToken, loadConversation, refreshAgents, refreshProjects, refreshConversations, setActiveId, setPendingTurns],
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

  function cancelTruthId() {
    truthIdAbort.current?.abort();
    setTruthIdQr(null);
    setPhase({ kind: "login" });
  }

  function refreshTruthId() {
    setTruthIdQr(null);
    void connect({ kind: "truthid" });
  }

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
    void refreshProjects(connection);
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
    setProjects([]);
    setProjectId("");
    setWorkdir("");
    setServerName(null);
    setUser(undefined);
    setChangingPassword(false);
    setRecoveryCode(null);
    setNoticeLater(false);
    setView("chat");
    setPhase({ kind: "login" });
  }

  function handleApproval(approvalId: number, approved: boolean, always = false) {
    setApprovals((queue) => queue.filter((p) => p.approvalId !== approvalId));
    try {
      connRef.current?.resolveApproval(approvalId, approved, always);
    } catch {
      // The connection is gone; the hub already counts an unanswered request as a no.
    }
  }

  function handleSend(message: string, attachments: Attachment[]) {
    const connection = connRef.current;
    if (!connection) return;
    const id = activeIdRef.current;
    const isCodeProject = projects.some((p) => p.id === projectId && p.code);
    const codeMode = codeModes[id] ?? "manual";
    const entry: ChatEntry = { role: "user", content: message, attachments };
    setEntries((current) => [...current, entry]);
    setPendingTurns((current) => ({ ...current, [id]: entry }));
    if (!conversations.some((c) => c.id === id)) {
      const now = Date.now();
      setConversations((current) => [
        { id, title: titleFrom(titleSeed(message, attachments)), createdAt: now, updatedAt: now, ...(agentId && { agentId }), ...(projectId && { projectId }), ...(!projectId && workdir && { workdir }) },
        ...current,
      ]);
    }
    try {
      // Said again with every task: a hub that restarted has forgotten it, and the picker must be what applies.
      if (isCodeProject) connection.setCodeMode(id, codeMode);
      // The project only counts when this turn starts the conversation: the hub keeps an existing one where it was made.
      // The folder, too: only the turn that starts the conversation can name one, and a project has its own.
      connection.sendChat(message, id, attachments, agentId || undefined, projectId || undefined, (!projectId && workdir) || undefined);
    } catch (err) {
      setPendingTurns(({ [id]: _failed, ...rest }) => rest);
      setEntries((current) => [...current, { role: "error", content: errorText(err), attachments: [] }]);
    }
  }

  /** Opens the folder browser (P102). The owner also gets the nodes to pick a folder on; a member's hub list already has
   * the node folders named for them, and they can't list nodes. */
  async function openFolderPicker() {
    const connection = connRef.current;
    if (connection && !user) {
      try {
        setKnownNodes(await connection.listNodes());
      } catch {
        // No nodes to offer: the hub's own folders are still there.
      }
    }
    setPickingFolder(true);
  }

  // A folder on a node is named by the node's name: asked once the owner opens a conversation that has one.
  useEffect(() => {
    const connection = connRef.current;
    if (!connection || user || knownNodes.length > 0 || !parseNodeFolder(workdir)) return;
    connection.listNodes().then(setKnownNodes).catch(() => {});
  }, [workdir, user, knownNodes.length]);

  /** The picker's choice, at once — a task that is running included, which is the point of changing it mid-way. */
  function handleCodeMode(mode: CodeMode) {
    const id = activeIdRef.current;
    setCodeModes((current) => ({ ...current, [id]: mode }));
    try {
      connRef.current?.setCodeMode(id, mode);
    } catch {
      // Offline: the next task says it again.
    }
  }

  /** Stops the code task this conversation is running (P103 b); the turn still ends with the engine's answer so far. */
  function handleCancel() {
    connRef.current?.cancelTurn(activeIdRef.current);
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
    setProjectId(conversationsRef.current.find((c) => c.id === id)?.projectId ?? "");
    setWorkdir(conversationsRef.current.find((c) => c.id === id)?.workdir ?? "");
    const connection = connRef.current;
    if (connection) void loadConversation(connection, id);
  }

  /** Uma conversa nova com `id`, a partir da árvore da organização. */
  function openChatWith(id: string) {
    setSidebarOpen(false);
    setView("chat");
    setActiveId(newConversationId());
    setEntries([]);
    setAgentId(id);
    setProjectId("");
    setWorkdir("");
  }

  function handleNewConversation() {
    setSidebarOpen(false);
    setView("chat");
    // Already on an empty, never-sent conversation — nothing to leave behind.
    if (!conversations.some((c) => c.id === activeIdRef.current) && entries.length === 0) return;
    setActiveId(newConversationId());
    setEntries([]);
    setAgentId("");
    setProjectId("");
    setWorkdir("");
  }

  function showView(next: View) {
    setView(next);
    // Agents may have been added or renamed in Settings meanwhile.
    if (next === "chat" && connRef.current) {
      void refreshAgents(connRef.current);
      void refreshProjects(connRef.current);
    }
  }

  /** The chat's project picker (P103). Before the conversation exists it only chooses where the first message goes;
   * on one that exists it moves it, after saying what that does to what was already said. */
  async function handleChooseProject(next: string) {
    const connection = connRef.current;
    const id = activeIdRef.current;
    if (!conversationsRef.current.some((c) => c.id === id)) {
      setProjectId(next);
      // A project has its own folder: the two are never both.
      if (next) setWorkdir("");
      return;
    }
    if (!connection || next === projectId) return;
    const target = projects.find((p) => p.id === next)?.name;
    const warning = target
      ? `Mover esta conversa para o projeto "${target}"? As próximas mensagens passam a valer só com os arquivos e as instruções dele. O que já foi dito continua na conversa.`
      : "Tirar esta conversa do projeto? O que já foi dito continua nela, e passa a fazer parte do contexto fora do projeto. As próximas mensagens voltam a ver o cofre inteiro.";
    if (!window.confirm(warning)) return;
    try {
      await connection.moveConversation(id, next || undefined);
      setProjectId(next);
      await refreshConversations(connection);
    } catch (err) {
      setConversationsError(`Não foi possível mover: ${errorText(err)}`);
    }
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
        truthIdQr={truthIdQr}
        onTruthIdCancel={cancelTruthId}
        onTruthIdRefresh={refreshTruthId}
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
        truthid={user.truthid}
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

  // P84 fatia 4 parte B: the workspace's recovery policy changed to a weaker one and needs their yes, or the
  // owner recovered their data — told before anything else, once their own password is in.
  const hasRecoveryNotice = user !== undefined && !user.mustChangePassword && ((user.policyPending ?? false) || (user.recoveries ?? []).some((r) => !r.seen));
  if (conn && user && hasRecoveryNotice && !noticeLater) {
    return (
      <RecoveryNoticeView
        conn={conn}
        user={user}
        onAccepted={(code) => {
          setUser((current) => (current ? { ...current, policyPending: false, memberPolicy: current.recoveryPolicy ?? current.memberPolicy } : current));
          if (code !== undefined) setRecoveryCode({ code, replacing: true });
        }}
        onAcked={() => setUser((current) => (current ? { ...current, recoveries: (current.recoveries ?? []).map((r) => ({ ...r, seen: true })) } : current))}
        onLater={() => setNoticeLater(true)}
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
          <button type="button" className={view === "projects" ? "tab tab--active" : "tab"} onClick={() => setView("projects")}>
            Projetos
          </button>
          {isOwner && (
            <>
              <button type="button" className={view === "organization" ? "tab tab--active" : "tab"} onClick={() => setView("organization")}>
                Organização
              </button>
              <button
                type="button"
                className={view === "agentWork" ? "tab tab--active" : "tab"}
                onClick={() => {
                  setAgentWorkFilter(null);
                  setView("agentWork");
                }}
              >
                Trabalho dos agentes
              </button>
              <button type="button" className={view === "tasks" ? "tab tab--active" : "tab"} onClick={() => setView("tasks")}>
                Tarefas
              </button>
              <button type="button" className={view === "webhooks" ? "tab tab--active" : "tab"} onClick={() => setView("webhooks")}>
                Webhooks
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
              projects={projects}
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
                {(projects.length > 0 || projectId !== "") && !(workdir && conversations.some((c) => c.id === activeId)) && (
                  // Chosen before the conversation's first message; after it, choosing moves the conversation (P103).
                  // Not on a conversation that works in a folder: it can't be moved into a project (P102).
                  <label className="agent-picker">
                    <span className="agent-picker-label">Projeto</span>
                    <select
                      value={projects.some((p) => p.id === projectId) ? projectId : ""}
                      onChange={(e) => void handleChooseProject(e.target.value)}
                      disabled={activeId in pendingTurns || !phase.connected}
                      title="A IA trabalha só nos arquivos do projeto, com as instruções dele. Numa conversa que já começou, escolher outro move a conversa."
                    >
                      <option value="">Nenhum</option>
                      {projects.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.name}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
                {!projectId && (conversations.some((c) => c.id === activeId) ? workdir !== "" : phase.connected) && (
                  // P102: the folder is picked before the first message and is only shown after it.
                  <div className="agent-picker">
                    <span className="agent-picker-label">Pasta</span>
                    {conversations.some((c) => c.id === activeId) ? (
                      <span className="folder-chip" title={folderPlace(workdir, knownNodes)}>
                        {folderLabel(workdir, knownNodes)}
                      </span>
                    ) : (
                      <button type="button" className="link-button folder-chip" onClick={() => void openFolderPicker()} title={workdir ? folderPlace(workdir, knownNodes) : "Escolher uma pasta do hub ou de um nó para a IA trabalhar"}>
                        {workdir ? folderLabel(workdir, knownNodes) : "Nenhuma"}
                      </button>
                    )}
                    {workdir && !conversations.some((c) => c.id === activeId) && (
                      <button type="button" className="link-button" onClick={() => setWorkdir("")} aria-label="Tirar a pasta">
                        ×
                      </button>
                    )}
                  </div>
                )}
                {projects.some((p) => p.id === projectId && p.code) && (
                  // Changeable at any moment, a task that is running included (P103 b).
                  <label className={`agent-picker${(codeModes[activeId] ?? "manual") === "acceptAll" ? " code-mode--warn" : ""}`}>
                    <span className="agent-picker-label">Modo</span>
                    <select
                      value={codeModes[activeId] ?? "manual"}
                      onChange={(e) => handleCodeMode(e.target.value as CodeMode)}
                      disabled={!phase.connected}
                      title="Quanto a IA pergunta antes de agir. Vale na hora, até no meio de uma tarefa; ao reabrir a conversa volta a Manual."
                    >
                      <option value="manual">Manual (pergunta tudo)</option>
                      <option value="acceptEdits">Aceitar edições</option>
                      <option value="acceptAll">Aceitar tudo (sem perguntar)</option>
                      <option value="plan">Plano (não altera nada)</option>
                    </select>
                  </label>
                )}
              </div>
              {pickingFolder && connRef.current && (
                <FolderPicker
                  listDirs={(path) => connRef.current!.listDirs(path)}
                  nodes={nodesWithFolders(knownNodes)}
                  initialPath={workdir || undefined}
                  onPick={(path) => {
                    setWorkdir(path);
                    setPickingFolder(false);
                  }}
                  onCancel={() => setPickingFolder(false)}
                />
              )}
              <ChatView
                entries={entries}
                pending={activeId in pendingTurns}
                live={liveTurns[activeId]}
                onCancel={handleCancel}
                disabled={!phase.connected}
                onSend={handleSend}
                onTranscribe={handleTranscribe}
                onExtendLimit={handleExtendLimit}
                onCycleMode={
                  projects.some((p) => p.id === projectId && p.code)
                    ? () => handleCodeMode(nextCodeMode(codeModes[activeId] ?? "manual"))
                    : undefined
                }
              />
            </div>
          </div>
        ) : view === "vault" ? (
          <VaultView conn={conn} />
        ) : view === "usage" ? (
          <UsageView conn={conn} />
        ) : view === "projects" ? (
          <ProjectsView conn={conn} onChanged={setProjects} />
        ) : view === "tasks" ? (
          <TasksView conn={conn} onOpenConversation={openConversation} />
        ) : view === "webhooks" ? (
          <WebhooksView conn={conn} onOpenConversation={openConversation} />
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
        ) : view === "agentWork" ? (
          <AgentTasksView conn={conn} agent={agentWorkFilter} onClearAgent={() => setAgentWorkFilter(null)} />
        ) : view === "organization" ? (
          <OrganizationView
            conn={conn}
            onEdit={() => setView("settings")}
            onOpenChat={openChatWith}
            onOpenTasks={(id) => {
              setAgentWorkFilter(id);
              setView("agentWork");
            }}
          />
        ) : (
          <SkillsView
            conn={conn}
            user={user}
            onLearningChange={(optOut) => setUser((current) => (current ? { ...current, learningOptOut: optOut } : current))}
          />
        )}
      </main>
      <ApprovalModal queue={approvals} onAnswer={handleApproval} />
    </div>
  );
}
