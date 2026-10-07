import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import ChatArea from "./components/ChatArea";
import HubConnectDialog from "./components/HubConnectDialog";
import HubSwitcher from "./components/HubSwitcher";
import Sidebar from "./components/Sidebar";
import SettingsView from "./components/SettingsView";
import UsageView from "./components/UsageView";
import OrganizationView from "./components/OrganizationView";
import AgentTasksView from "./components/AgentTasksView";
import SkillsView from "./components/SkillsView";
import ProjectsView from "./components/ProjectsView";
import ApprovalModal from "./components/ApprovalModal";
import SyncView from "./components/SyncView";
import TasksView from "./components/TasksView";
import WebhooksView from "./components/WebhooksView";
import VaultView from "./components/VaultView";
import WorkspaceView from "./components/WorkspaceView";
import { applyEvent, type ChatEventDto, type LiveTurn } from "./lib/liveTurn";
import {
  HubTurnError,
  hubAgents,
  hubChat,
  hubConnect,
  hubDisconnect,
  hubHistory,
  hubListConversations,
  hubListDirs,
  hubListNodes,
  hubListProjects,
  hubMoveConversation,
  hubSend,
  needsSignIn,
  type HubCredential,
  type RemoteState,
  type RemoteStatePayload,
} from "./lib/hub";
import { decorateLastAnswer, mergeConversations } from "./lib/hubMap";
import { threadHistory, threadsOf, visibleConversations } from "./lib/threads";
import ThreadPanel, { LocalThreadPanel } from "./components/ThreadPanel";
import { parseNodeFolder, type NodeInfo } from "./lib/workdir";
import type { Attachment, ChatMessage, CodeMode, Conversation, ProjectEntry, ProviderFallback, SavedHub, Settings, Usage } from "./types";

const emptySettings: Settings = {
  providers: [],
  activeProvider: "",
  combos: [],
  modelPolicies: [],
  vaultPath: "",
  generatedPath: "",
  tavilyKey: "",
  whisperKey: "",
  enableShell: false,
  defaultModels: {},
  mcpServers: [],
  agents: [],
  sshHosts: [],
  gitSync: null,
  limits: null,
  defaultLimits: [],
  limitsDisabledByEnv: false,
  prices: [],
  version: "",
};

/** Whether `id` is a model a conversation can use: a provider or a combo (P90). */
function isModel(settings: Settings, id: string): boolean {
  return settings.providers.some((p) => p.id === id) || settings.combos.some((c) => c.id === id);
}

// Purely a per-device UI preference (not something another device/channel needs to know about),
// so localStorage rather than config.toml is the right home for it.
const SIDEBAR_COLLAPSED_KEY = "warden.sidebarCollapsed";

function titleFromMessage(content: string): string {
  const collapsed = content.trim().replace(/\s+/g, " ");
  return collapsed.length > 40 ? `${collapsed.slice(0, 40)}…` : collapsed;
}

/** What a turn of a thread on this computer needs besides the message (P125): the agent picked in the thread, the message it comes from, and
 * the project and folder of the conversation it came from (a thread works where that one does). */
interface LocalThread {
  agentId: string;
  parent: { conversationId: string; messageId: string };
  projectId: string;
  workdir: string;
}

/** Puts the copy of a conversation just read from disk in the list (P87). It wins, since it has
 * what other writers added, but a message this screen shows and the disk doesn't have yet (its own
 * append still in flight) stays at the end, and the fallback notice (shown only, never saved) is
 * kept on the message it belongs to. */
function replaceWithSaved(conversations: Conversation[], saved: Conversation): Conversation[] {
  const local = conversations.find((c) => c.id === saved.id);
  const localById = new Map((local?.messages ?? []).map((m) => [m.id, m]));
  const savedIds = new Set(saved.messages.map((m) => m.id));
  const merged: Conversation = {
    ...saved,
    messages: [
      ...saved.messages.map((m) => {
        const fallbacks = localById.get(m.id)?.fallbacks;
        return fallbacks ? { ...m, fallbacks } : m;
      }),
      ...(local?.messages ?? []).filter((m) => !savedIds.has(m.id)),
    ],
  };
  return [merged, ...conversations.filter((c) => c.id !== saved.id)].sort((a, b) => b.updatedAt - a.updatedAt);
}

function App() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  // What a code engine has done so far in the task each conversation is running (P103 b), until the turn ends.
  const [liveTurns, setLiveTurns] = useState<Record<string, LiveTurn>>({});
  // How much each code conversation asks (P103 b), by conversation id — "new" for one that hasn't started. Only here,
  // never saved: a conversation opened again asks everything.
  const [codeModes, setCodeModes] = useState<Record<string, CodeMode>>({});
  const [view, setView] = useState<"chat" | "settings" | "usage" | "sync" | "vault" | "skills" | "projects" | "tasks" | "webhooks" | "workspace" | "organization" | "agentWork">("chat");
  const [settings, setSettings] = useState<Settings>(emptySettings);
  const [selectedAgentId, setSelectedAgentId] = useState("");
  /** The agent a conversation that isn't started yet speaks with (set from the organization tree), so choosing it survives the
   * reload of the settings that opening the chat triggers. Empty once a conversation is open. */
  const [newChatAgent, setNewChatAgent] = useState("");
  /** The agent whose tasks the "Agent work" screen is narrowed to (set from the organization tree). */
  const [agentWorkFilter, setAgentWorkFilter] = useState<string | null>(null);
  const [selectedProviderId, setSelectedProviderId] = useState("");
  const [projects, setProjects] = useState<ProjectEntry[]>([]);
  // The project a *new* conversation will start in (P103); an existing one has its own, fixed at creation.
  const [selectedProjectId, setSelectedProjectId] = useState("");
  // The folder of this computer a *new* conversation will work in (P102); an existing one has its own. Never with a project.
  const [selectedWorkdir, setSelectedWorkdir] = useState("");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true");
  // P102 phase 2: the machine the screens use. `null` is this computer; otherwise a saved hub, whose conversations,
  // projects and agents these screens show and whose engine answers (the connection itself lives in Rust).
  const [hubs, setHubs] = useState<SavedHub[]>([]);
  const [activeHubId, setActiveHubId] = useState<string | null>(null);
  const [hubStates, setHubStates] = useState<Record<string, RemoteState>>({});
  const [connectingTo, setConnectingTo] = useState<SavedHub | null>(null);
  const [switching, setSwitching] = useState(false);
  const remote = activeHubId !== null;
  const remoteReady = activeHubId !== null && hubStates[activeHubId]?.state === "connected";
  // The hub's nodes (P102), to pick a folder on one: only the owner can list them, a member has none to read.
  const [knownNodes, setKnownNodes] = useState<NodeInfo[]>([]);
  const hubState = activeHubId ? hubStates[activeHubId] : undefined;
  const isHubOwner = hubState?.state === "connected" && hubState.user === null;
  // What the listeners (set up once) need to know about the machine in use.
  const activeHubRef = useRef<string | null>(null);
  activeHubRef.current = activeHubId;
  const activeConversationRef = useRef<string | null>(null);
  activeConversationRef.current = activeConversationId;
  // Conversations started on this screen that the hub's list doesn't have yet: kept on top until it does.
  const startedHere = useRef(new Set<string>());

  const activeConversation = conversations.find((c) => c.id === activeConversationId);
  // A thread open beside the chat (P125, on a hub or on this computer): the message it comes from and the conversation it lives in. It belongs to the
  // conversation it was opened in, so moving to another conversation or machine closes it.
  const [thread, setThread] = useState<{ messageId: string; threadId: string } | null>(null);
  useEffect(() => setThread(null), [activeConversationId, activeHubId]);
  // On this computer the turn of the thread is run by this screen (P125); on a hub the panel does it.
  const [threadSending, setThreadSending] = useState(false);
  const [threadError, setThreadError] = useState<string | null>(null);
  useEffect(() => setThreadError(null), [thread]);
  function openThread(messageId: string) {
    if (activeConversationId === null) return;
    const known = threadsOf(conversations, activeConversationId)[messageId];
    setThread({ messageId, threadId: known?.conversationId ?? crypto.randomUUID() });
  }
  const currentProjectId = activeConversation ? (activeConversation.projectId ?? "") : selectedProjectId;
  const currentWorkdir = currentProjectId ? "" : activeConversation ? (activeConversation.workdir ?? "") : selectedWorkdir;

  function loadProjects() {
    const load = activeHubRef.current === null ? invoke<ProjectEntry[]>("list_projects") : hubListProjects();
    load.then(setProjects).catch((err) => console.error("failed to load projects:", err));
  }

  /** What the chat header's pickers offer, from the machine in use. A hub has no per-turn model (the agent decides), so
   * only its agents come. */
  function loadSettings(): Promise<void> {
    const load =
      activeHubRef.current === null
        ? invoke<Settings>("get_settings")
        : hubAgents().then((agents): Settings => ({ ...emptySettings, agents }));
    return load.then(setSettings).catch((err) => console.error("failed to load settings:", err));
  }

  /** The conversation list of the machine in use. On a hub it has no messages: they come when one is opened. */
  function loadConversations(): Promise<void> {
    if (activeHubRef.current === null) {
      return invoke<Conversation[]>("list_conversations")
        .then(setConversations)
        .catch((err) => console.error("failed to load conversation history:", err));
    }
    return hubListConversations()
      .then((summaries) => setConversations((prev) => mergeConversations(prev, summaries, startedHere.current)))
      .catch((err) => console.error("failed to load the hub's conversations:", err));
  }

  /** The hub's nodes, for the folder browser and for naming a folder that is on one. A hub with none, or a member (who can't
   * ask), leaves it empty: the hub's own folders are still there. */
  async function loadNodes() {
    if (!isHubOwner) return;
    try {
      setKnownNodes(await hubListNodes());
    } catch {
      // No nodes to offer.
    }
  }

  // A folder on a node is named by the node's name: read once the owner opens a conversation that has one.
  useEffect(() => {
    if (isHubOwner && knownNodes.length === 0 && parseNodeFolder(currentWorkdir)) void loadNodes();
  }, [currentWorkdir, activeHubId, isHubOwner]);

  /** A hub conversation's messages, from its history. */
  function loadHistory(conversationId: string) {
    hubHistory(conversationId)
      .then((messages) => setConversations((prev) => prev.map((c) => (c.id === conversationId ? { ...c, messages } : c))))
      .catch((err) => console.error("failed to load the conversation:", err));
  }

  // The list of the machine in use, loaded on start and again when the machine changes or a hub comes (back) up.
  useEffect(() => {
    if (remote && !remoteReady) return;
    loadConversations();
  }, [activeHubId, remoteReady]);

  // The saved hubs, for the machine picker: read again on coming back from the Workspace screen, where they are edited.
  useEffect(() => {
    if (view !== "chat") return;
    invoke<SavedHub[]>("list_hubs")
      .then(setHubs)
      .catch((err) => console.error("failed to load the saved hubs:", err));
  }, [view]);

  // What the connection to each hub says about itself (connecting, connected as whom, retrying, stopped).
  useEffect(() => {
    const unlisten = listen<RemoteStatePayload>("remote-hub-state", ({ payload }) => {
      setHubStates((prev) => ({ ...prev, [payload.hubId]: payload.state }));
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // A project made, edited or removed in its screen shows up in the sidebar and the picker on coming back.
  useEffect(() => {
    if (view === "chat" && (!remote || remoteReady)) loadProjects();
  }, [view, activeHubId, remoteReady]);

  // An agent left a message for another, or answered one (P46 `message_agent`): the backend wrote
  // that conversation to disk, so take the saved copy of it — only it, the rest stays as is here.
  useEffect(() => {
    const unlisten = listen<string>("conversations-changed", (event) => {
      if (activeHubRef.current !== null) {
        // On a hub: the list again, and the open conversation's messages if it is the one that changed.
        loadConversations();
        if (event.payload === activeConversationRef.current) loadHistory(event.payload);
        return;
      }
      invoke<Conversation[]>("list_conversations")
        .then((saved) => {
          const changed = saved.find((c) => c.id === event.payload);
          if (!changed) return;
          setConversations((prev) => replaceWithSaved(prev, changed));
        })
        .catch((err) => console.error("failed to reload a conversation:", err));
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // The backend tells what the code engine does as it does it (P103 b).
  useEffect(() => {
    const unlisten = listen<{ conversationId: string; event: ChatEventDto }>("chat-event", ({ payload }) => {
      setLiveTurns((prev) => ({ ...prev, [payload.conversationId]: applyEvent(prev[payload.conversationId], payload.event) }));
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // Settings (providers/agents) only has a UI to edit itself in the Settings screen — refetch
  // whenever the user comes back from there so the chat header's selectors stay in sync without
  // needing an app restart.
  useEffect(() => {
    if (view !== "chat" || (remote && !remoteReady)) return;
    void loadSettings();
  }, [view, activeHubId, remoteReady]);

  // Restores the agent/model this conversation was last using (P3) whenever it's switched, or
  // falls back to defaults if that id no longer matches anything configured (deleted since).
  useEffect(() => {
    const storedAgentId = activeConversation?.agentId ?? (activeConversationId === null ? newChatAgent : "");
    setSelectedAgentId(settings.agents.some((a) => a.id === storedAgentId) ? storedAgentId : "");

    const storedProviderId = activeConversation?.providerId ?? "";
    setSelectedProviderId(isModel(settings, storedProviderId) ? storedProviderId : settings.activeProvider);
  }, [activeConversationId, settings, newChatAgent]);

  /** Moves the open conversation into a project, or out of any with "" (P103); the saved copy replaces the one shown. */
  async function handleMoveProject(projectId: string) {
    if (activeConversationId === null) return;
    setSendError(null);
    if (remote) {
      try {
        await hubMoveConversation(activeConversationId, projectId);
        loadConversations();
      } catch (err) {
        setSendError(String(err));
      }
      return;
    }
    try {
      const saved = await invoke<Conversation>("move_conversation", { conversationId: activeConversationId, projectId: projectId || null });
      setConversations((prev) => replaceWithSaved(prev, saved));
    } catch (err) {
      setSendError(String(err));
    }
  }

  function handleToggleSidebarCollapsed() {
    setSidebarCollapsed((prev) => {
      const next = !prev;
      localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(next));
      return next;
    });
  }

  /** A new conversation with `agentId`, from the organization tree. */
  function openChatWith(agentId: string) {
    setNewChatAgent(agentId);
    setActiveConversationId(null);
    setSelectedProjectId("");
    setSelectedWorkdir("");
    setView("chat");
  }

  function handleSelectAgent(agentId: string) {
    if (activeConversationId === null) setNewChatAgent(agentId);
    setSelectedAgentId(agentId);
    // Pre-fills the model selector with the agent's default, if it has one — the user can still
    // change it afterward, this is just a convenience.
    const agent = settings.agents.find((a) => a.id === agentId);
    if (agent?.providerId && isModel(settings, agent.providerId)) {
      setSelectedProviderId(agent.providerId);
    }
  }

  /** `persist` false only shows the message: a code project's turn saves the whole exchange itself (P103 b). */
  function appendMessage(conversationId: string, message: ChatMessage, titleSeed?: string, persist = true, thread?: LocalThread) {
    // A thread (P125) speaks as the agent picked in it, and works in the project and folder of the conversation it came from.
    const agentId = (thread ? thread.agentId : selectedAgentId) || undefined;
    const providerId = selectedProviderId || undefined;
    const projectId = (thread ? thread.projectId : currentProjectId) || undefined;
    const workdir = (thread ? thread.workdir : currentWorkdir) || undefined;
    // Shown at once; the saved copy then replaces it (P87).
    setConversations((prev) => {
      const existing = prev.find((c) => c.id === conversationId);
      const conversation: Conversation = existing
        ? { ...existing, messages: [...existing.messages, message], updatedAt: message.createdAt, agentId, providerId }
        : {
            id: conversationId,
            title: titleFromMessage(titleSeed ?? message.content),
            messages: [message],
            createdAt: message.createdAt,
            updatedAt: message.createdAt,
            agentId,
            providerId,
            projectId,
            workdir,
            ...(thread ? { parent: thread.parent } : {}),
          };
      return existing ? prev.map((c) => (c.id === conversationId ? conversation : c)) : [conversation, ...prev];
    });

    if (!persist) return;

    // Appended on disk rather than saving the whole conversation held here: another writer (an agent
    // answering a note in this conversation, the CLI) may have added to it meanwhile (P87).
    const { fallbacks: _shownOnly, ...saved } = message;
    invoke<Conversation>("append_conversation_messages", {
      conversationId,
      messages: [saved],
      titleSeed: titleSeed ?? message.content,
      agentId: agentId ?? null,
      providerId: providerId ?? null,
      // Only counts when this append creates the conversation: an existing one keeps the project it was made in.
      projectId: projectId ?? null,
      // Same rule for the folder (P102); a conversation in a project has none.
      workdir: workdir ?? null,
      // Same rule for the message a thread comes from (P125).
      threadOf: thread?.parent ?? null,
    })
      .then((onDisk) => setConversations((prev) => replaceWithSaved(prev, onDisk)))
      .catch((err) => console.error("failed to persist conversation:", err));
  }

  const codeMode = codeModes[activeConversationId ?? "new"] ?? "manual";

  /** The picker's choice, at once — a task that is running included, which is the point of changing it mid-way. */
  function handleCodeMode(mode: CodeMode) {
    setCodeModes((prev) => ({ ...prev, [activeConversationId ?? "new"]: mode }));
    if (activeConversationId !== null) {
      const sent = remote ? hubSend({ type: "setCodeMode", conversationId: activeConversationId, mode }) : invoke("set_code_mode", { conversationId: activeConversationId, mode });
      sent.catch((err) => setSendError(String(err)));
    }
  }

  /** Leaves what the previous machine showed, so nothing of it lingers under the new one's name. */
  function resetForMachine() {
    setActiveConversationId(null);
    setConversations([]);
    setProjects([]);
    setSettings(emptySettings);
    setSelectedProjectId("");
    setSelectedWorkdir("");
    setSendError(null);
    setLiveTurns({});
    setKnownNodes([]);
    startedHere.current.clear();
  }

  /** The machine picker: this computer, or a saved hub (asks how to sign in when there is no token for it yet). */
  async function handlePickMachine(hubId: string | null) {
    if (hubId === activeHubId) return;
    setSendError(null);
    setSwitching(true);
    try {
      if (hubId === null) {
        await hubDisconnect();
        resetForMachine();
        setActiveHubId(null);
        return;
      }
      try {
        await hubConnect(hubId);
        resetForMachine();
        setActiveHubId(hubId);
      } catch (err) {
        if (needsSignIn(err)) setConnectingTo(hubs.find((h) => h.id === hubId) ?? null);
        else setSendError(String(err));
      }
    } finally {
      setSwitching(false);
    }
  }

  /** The sign-in dialog's submit: rejects with the reason, which the dialog shows. */
  async function handleSignIn(credential: HubCredential) {
    if (!connectingTo) return;
    const hubId = connectingTo.id;
    await hubConnect(hubId, credential);
    setConnectingTo(null);
    resetForMachine();
    setActiveHubId(hubId);
  }

  /** Opens a conversation. On a hub its messages are read again from there: another device may have added to them. */
  function selectConversation(id: string) {
    setActiveConversationId(id);
    setView("chat");
    if (remote) loadHistory(id);
  }

  /** A turn on a hub (P102): it keeps the conversation, so nothing is saved from here and its copy replaces what is shown. */
  async function handleSendRemote(content: string, attachments: Attachment[]) {
    if (!remoteReady) {
      setSendError("Not connected to the hub right now.");
      return;
    }
    const conversationId = activeConversationId ?? crypto.randomUUID();
    // Only the message that creates the conversation carries its project or folder.
    const creating = activeConversation === undefined;
    const userMessage: ChatMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content,
      createdAt: Date.now(),
      ...(attachments.length > 0 ? { attachments } : {}),
    };
    const isCodeTurn = projects.some((p) => p.id === currentProjectId && p.code);
    appendMessage(conversationId, userMessage, content || "Image", false);
    if (creating) startedHere.current.add(conversationId);
    if (activeConversationId === null) {
      setCodeModes(({ new: _started, ...rest }) => ({ ...rest, [conversationId]: codeMode }));
      setActiveConversationId(conversationId);
    }

    setSendError(null);
    setIsSending(true);
    try {
      // The hub forgets a conversation's mode when it restarts, so it is said with every code task.
      if (isCodeTurn) await hubSend({ type: "setCodeMode", conversationId, mode: codeMode });
      const reply = await hubChat({
        content,
        attachments,
        conversationId,
        agentId: selectedAgentId,
        projectId: currentProjectId,
        workdir: currentWorkdir,
        creating,
      });
      const messages = decorateLastAnswer(await hubHistory(conversationId), reply);
      setConversations((prev) => prev.map((c) => (c.id === conversationId ? { ...c, messages } : c)));
    } catch (err) {
      setSendError(err instanceof HubTurnError ? err.message : String(err));
    } finally {
      setIsSending(false);
      setLiveTurns(({ [conversationId]: _done, ...rest }) => rest);
      // The hub's title and order for it; once it lists the conversation it no longer needs to be kept here.
      void loadConversations().finally(() => startedHere.current.delete(conversationId));
      void loadSettings();
    }
  }

  async function handleSendMessage(content: string, attachments: Attachment[] = []) {
    if (remote) return handleSendRemote(content, attachments);
    const conversationId = activeConversationId ?? crypto.randomUUID();
    const history = (activeConversation?.messages ?? []).map(({ role, content, attachments }) => ({
      role,
      content,
      attachments: attachments ?? [],
    }));
    const userMessage: ChatMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content,
      createdAt: Date.now(),
      ...(attachments.length > 0 ? { attachments } : {}),
    };

    // A code project's turn saves the exchange itself, with the engine's session; saving the message here first would
    // leave it twice in the file.
    const isCodeTurn = projects.some((p) => p.id === currentProjectId && p.code);
    appendMessage(conversationId, userMessage, content || "Image", !isCodeTurn);
    if (activeConversationId === null) {
      // The mode picked before the first message goes with the conversation that message starts.
      setCodeModes(({ new: _started, ...rest }) => ({ ...rest, [conversationId]: codeMode }));
      setActiveConversationId(conversationId);
    }

    setSendError(null);
    setIsSending(true);
    try {
      const reply = await invoke<{
        content: string;
        usage?: Usage;
        attachments?: Attachment[];
        generatedFiles?: string[];
        fallbacks?: ProviderFallback[];
        alreadySaved?: boolean;
      }>("send_message", {
        history,
        content,
        attachments,
        agentId: selectedAgentId || null,
        // Only sent as an override when it actually differs from the active provider — the base
        // orchestrator already uses that one, no need to rebuild a `ModelProvider` for it.
        providerId: selectedProviderId && selectedProviderId !== settings.activeProvider ? selectedProviderId : null,
        // The conversation's project (P103): the turn runs on its folder, with its instructions.
        projectId: currentProjectId || null,
        // The conversation's folder (P102): `read_file`, `write_file` and the shell act there.
        workdir: currentWorkdir || null,
        conversationId,
        // Said with every task, so the mode the picker shows is the one that applies.
        codeMode: isCodeTurn ? codeMode : null,
      });
      if (reply.alreadySaved) {
        // The backend wrote both messages: take the saved copy instead of appending.
        const saved = (await invoke<Conversation[]>("list_conversations")).find((c) => c.id === conversationId);
        if (saved) setConversations((prev) => replaceWithSaved(prev, saved));
        return;
      }
      appendMessage(conversationId, {
        id: crypto.randomUUID(),
        role: "assistant",
        content: reply.content,
        createdAt: Date.now(),
        usage: reply.usage,
        // Media an MCP tool produced this turn (P64 frente 2) — omitted entirely when empty, same
        // convention as the user message's `attachments` above.
        ...(reply.attachments && reply.attachments.length > 0 ? { attachments: reply.attachments } : {}),
        // Files actually written to disk this turn (P64) — same omit-when-empty convention.
        ...(reply.generatedFiles && reply.generatedFiles.length > 0 ? { generatedFiles: reply.generatedFiles } : {}),
        // A reserve answered in place of the conversation's provider (P79) — same convention.
        ...(reply.fallbacks && reply.fallbacks.length > 0 ? { fallbacks: reply.fallbacks } : {}),
      });
    } catch (err) {
      setSendError(String(err));
    } finally {
      setIsSending(false);
      setLiveTurns(({ [conversationId]: _done, ...rest }) => rest);
      // A turn can add agents (an agent with "can create and edit other agents"), so the selector in
      // the chat header would otherwise stay stale until the next visit to Settings. Kept as the same
      // object when nothing changed: the effect above re-selects agent/model whenever `settings` changes.
      void invoke<Settings>("get_settings")
        .then((next) => setSettings((prev) => (JSON.stringify(prev) === JSON.stringify(next) ? prev : next)))
        .catch((err) => console.error("failed to refresh settings:", err));
    }
  }

  /** A turn in the thread open beside the conversation, on this computer (P125). The thread is a conversation of its own, saved with the link
   * to the message it came from; the model sees the conversation up to that message and then the thread. It works in the project and folder
   * of the conversation it came from and speaks as the agent picked in the thread. */
  async function handleSendLocalThread(content: string, attachments: Attachment[], agentId: string) {
    if (!thread || !activeConversation) return;
    const threadId = thread.threadId;
    const scope: LocalThread = {
      agentId,
      parent: { conversationId: activeConversation.id, messageId: thread.messageId },
      projectId: activeConversation.projectId ?? "",
      workdir: activeConversation.workdir ?? "",
    };
    const history = threadHistory(activeConversation, thread.messageId, conversations.find((c) => c.id === threadId)?.messages ?? []).map(
      ({ role, content, attachments }) => ({ role, content, attachments: attachments ?? [] }),
    );
    appendMessage(
      threadId,
      { id: crypto.randomUUID(), role: "user", content, createdAt: Date.now(), ...(attachments.length > 0 ? { attachments } : {}) },
      content || "Image",
      true,
      scope,
    );
    setThreadError(null);
    setThreadSending(true);
    try {
      const reply = await invoke<{ content: string; usage?: Usage; attachments?: Attachment[]; generatedFiles?: string[]; fallbacks?: ProviderFallback[] }>("send_message", {
        history,
        content,
        attachments,
        agentId: agentId || null,
        providerId: selectedProviderId && selectedProviderId !== settings.activeProvider ? selectedProviderId : null,
        projectId: scope.projectId || null,
        workdir: scope.workdir || null,
        conversationId: threadId,
        codeMode: null,
      });
      appendMessage(
        threadId,
        {
          id: crypto.randomUUID(),
          role: "assistant",
          content: reply.content,
          createdAt: Date.now(),
          usage: reply.usage,
          ...(reply.attachments && reply.attachments.length > 0 ? { attachments: reply.attachments } : {}),
          ...(reply.generatedFiles && reply.generatedFiles.length > 0 ? { generatedFiles: reply.generatedFiles } : {}),
          ...(reply.fallbacks && reply.fallbacks.length > 0 ? { fallbacks: reply.fallbacks } : {}),
        },
        undefined,
        true,
        scope,
      );
    } catch (err) {
      setThreadError(String(err));
    } finally {
      setThreadSending(false);
    }
  }

  /** The Stop button: the turn ends on its own, with what the engine had by then or an error to show. */
  function handleCancel() {
    if (activeConversationId === null) return;
    // A hub can only stop a code project's task; an ordinary turn there runs to its answer.
    const sent = remote ? hubSend({ type: "cancelTurn", conversationId: activeConversationId }) : invoke("cancel_turn", { conversationId: activeConversationId });
    sent.catch((err) => setSendError(String(err)));
  }

  return (
    <div className={`app-shell${sidebarCollapsed ? " app-shell--sidebar-collapsed" : ""}`}>
      <Sidebar
        conversations={visibleConversations(conversations)}
        projects={projects}
        activeConversationId={activeConversationId}
        onSelectConversation={selectConversation}
        onNewConversation={() => {
          setNewChatAgent("");
          setActiveConversationId(null);
          setSelectedProjectId("");
          setSelectedWorkdir("");
          setView("chat");
        }}
        onOpenSettings={() => setView("settings")}
        onOpenUsage={() => setView("usage")}
        onOpenSync={() => setView("sync")}
        onOpenVault={() => setView("vault")}
        onOpenSkills={() => setView("skills")}
        onOpenProjects={() => setView("projects")}
        onOpenTasks={() => setView("tasks")}
        onOpenWebhooks={() => setView("webhooks")}
        onOpenWorkspace={() => setView("workspace")}
        onOpenOrganization={() => setView("organization")}
        onOpenAgentWork={() => {
          setAgentWorkFilter(null);
          setView("agentWork");
        }}
        view={view}
        collapsed={sidebarCollapsed}
        onToggleCollapsed={handleToggleSidebarCollapsed}
        machine={
          <HubSwitcher
            hubs={hubs}
            activeHubId={activeHubId}
            state={activeHubId ? hubStates[activeHubId] : undefined}
            busy={switching}
            onPick={(id) => void handlePickMachine(id)}
            onSignIn={setConnectingTo}
          />
        }
      />
      {view === "settings" ? (
        <SettingsView />
      ) : view === "usage" ? (
        <UsageView key={activeHubId ?? "local"} remote={remote} />
      ) : view === "sync" ? (
        <SyncView />
      ) : view === "vault" ? (
        <VaultView key={activeHubId ?? "local"} remote={remote} />
      ) : view === "skills" ? (
        <SkillsView key={activeHubId ?? "local"} remote={remote} agents={settings.agents} providers={settings.providers} activeProvider={settings.activeProvider} />
      ) : view === "projects" ? (
        <ProjectsView onChanged={loadProjects} />
      ) : view === "tasks" ? (
        <TasksView key={activeHubId ?? "local"} remote={remote} agents={settings.agents} />
      ) : view === "webhooks" ? (
        <WebhooksView key={activeHubId ?? "local"} remote={remote} hubUrl={hubs.find((h) => h.id === activeHubId)?.url} agents={settings.agents} />
      ) : view === "workspace" ? (
        <WorkspaceView />
      ) : view === "agentWork" ? (
        <AgentTasksView key={activeHubId ?? "local"} remote={remote} agent={agentWorkFilter} onClearAgent={() => setAgentWorkFilter(null)} />
      ) : view === "organization" ? (
        <OrganizationView key={activeHubId ?? "local"} agents={settings.agents} remote={remote} onChanged={() => void loadSettings()} onEdit={() => setView("settings")} onOpenChat={openChatWith} onOpenTasks={(id) => { setAgentWorkFilter(id); setView("agentWork"); }} />
      ) : (
        <div className="chat-with-thread">
        <ChatArea
          activeConversation={activeConversation}
          onSendMessage={handleSendMessage}
          isSending={isSending}
          live={activeConversationId === null ? undefined : liveTurns[activeConversationId]}
          onCancel={handleCancel}
          codeMode={codeMode}
          onCodeMode={handleCodeMode}
          sendError={sendError}
          agents={settings.agents}
          providers={settings.providers}
          combos={settings.combos}
          projects={projects}
          selectedProjectId={currentProjectId}
          onSelectProject={(id) => {
            setSelectedProjectId(id);
            // A project has its own folder: the two are never both (P102).
            if (id) setSelectedWorkdir("");
          }}
          selectedWorkdir={currentWorkdir}
          onSelectWorkdir={setSelectedWorkdir}
          onMoveProject={(id) => void handleMoveProject(id)}
          selectedAgentId={selectedAgentId}
          selectedProviderId={selectedProviderId}
          onSelectAgent={handleSelectAgent}
          onSelectProvider={setSelectedProviderId}
          onOpenSettings={() => setView("settings")}
          remote={remote}
          hubFolders={remote ? { listDirs: hubListDirs, nodes: knownNodes, prepare: loadNodes } : undefined}
          // A thread needs a conversation that is saved, and a code project's context is the engine's session, not its messages.
          onOpenThread={activeConversation && !projects.some((p) => p.id === currentProjectId && p.code) ? openThread : undefined}
          threads={activeConversationId === null ? undefined : threadsOf(conversations, activeConversationId)}
        />
        {remote && thread && activeConversation && activeConversation.messages.some((m) => m.hubId === thread.messageId) && (
          <ThreadPanel
            key={thread.threadId}
            threadId={thread.threadId}
            parent={{ conversationId: activeConversation.id, messageId: thread.messageId }}
            anchor={activeConversation.messages.find((m) => m.hubId === thread.messageId)!}
            agentIds={settings.agents.map((a) => a.id)}
            initialAgentId={conversations.find((c) => c.id === thread.threadId)?.agentId ?? selectedAgentId}
            ready={remoteReady}
            onClose={() => setThread(null)}
            onChanged={() => void loadConversations()}
          />
        )}
        {!remote && thread && activeConversation && activeConversation.messages.some((m) => m.id === thread.messageId) && (
          <LocalThreadPanel
            key={thread.threadId}
            threadId={thread.threadId}
            anchor={activeConversation.messages.find((m) => m.id === thread.messageId)!}
            messages={conversations.find((c) => c.id === thread.threadId)?.messages ?? []}
            sending={threadSending}
            error={threadError}
            agentIds={settings.agents.map((a) => a.id)}
            initialAgentId={conversations.find((c) => c.id === thread.threadId)?.agentId ?? selectedAgentId}
            onSend={(content, attachments, agentId) => void handleSendLocalThread(content, attachments, agentId)}
            onClose={() => setThread(null)}
          />
        )}
        </div>
      )}
      {connectingTo &&<HubConnectDialog hub={connectingTo} onSubmit={handleSignIn} onCancel={() => setConnectingTo(null)} />}
      <ApprovalModal />
    </div>
  );
}

export default App;
