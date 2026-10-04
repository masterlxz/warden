import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import ChatArea from "./components/ChatArea";
import Sidebar from "./components/Sidebar";
import SettingsView from "./components/SettingsView";
import UsageView from "./components/UsageView";
import SkillsView from "./components/SkillsView";
import ProjectsView from "./components/ProjectsView";
import ApprovalModal from "./components/ApprovalModal";
import SyncView from "./components/SyncView";
import TasksView from "./components/TasksView";
import VaultView from "./components/VaultView";
import WorkspaceView from "./components/WorkspaceView";
import { applyEvent, type ChatEventDto, type LiveTurn } from "./lib/liveTurn";
import type { Attachment, ChatMessage, Conversation, ProjectEntry, ProviderFallback, Settings, Usage } from "./types";

const emptySettings: Settings = {
  providers: [],
  activeProvider: "",
  combos: [],
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
  const [view, setView] = useState<"chat" | "settings" | "usage" | "sync" | "vault" | "skills" | "projects" | "tasks" | "workspace">("chat");
  const [settings, setSettings] = useState<Settings>(emptySettings);
  const [selectedAgentId, setSelectedAgentId] = useState("");
  const [selectedProviderId, setSelectedProviderId] = useState("");
  const [projects, setProjects] = useState<ProjectEntry[]>([]);
  // The project a *new* conversation will start in (P103); an existing one has its own, fixed at creation.
  const [selectedProjectId, setSelectedProjectId] = useState("");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true");

  const activeConversation = conversations.find((c) => c.id === activeConversationId);
  const currentProjectId = activeConversation ? (activeConversation.projectId ?? "") : selectedProjectId;

  function loadProjects() {
    invoke<ProjectEntry[]>("list_projects")
      .then(setProjects)
      .catch((err) => console.error("failed to load projects:", err));
  }

  useEffect(() => {
    invoke<Conversation[]>("list_conversations")
      .then(setConversations)
      .catch((err) => console.error("failed to load conversation history:", err));
  }, []);

  // A project made, edited or removed in its screen shows up in the sidebar and the picker on coming back.
  useEffect(() => {
    if (view === "chat") loadProjects();
  }, [view]);

  // An agent left a message for another, or answered one (P46 `message_agent`): the backend wrote
  // that conversation to disk, so take the saved copy of it — only it, the rest stays as is here.
  useEffect(() => {
    const unlisten = listen<string>("conversations-changed", (event) => {
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
    if (view !== "chat") return;
    invoke<Settings>("get_settings")
      .then(setSettings)
      .catch((err) => console.error("failed to load settings:", err));
  }, [view]);

  // Restores the agent/model this conversation was last using (P3) whenever it's switched, or
  // falls back to defaults if that id no longer matches anything configured (deleted since).
  useEffect(() => {
    const storedAgentId = activeConversation?.agentId ?? "";
    setSelectedAgentId(settings.agents.some((a) => a.id === storedAgentId) ? storedAgentId : "");

    const storedProviderId = activeConversation?.providerId ?? "";
    setSelectedProviderId(isModel(settings, storedProviderId) ? storedProviderId : settings.activeProvider);
  }, [activeConversationId, settings]);

  /** Moves the open conversation into a project, or out of any with "" (P103); the saved copy replaces the one shown. */
  async function handleMoveProject(projectId: string) {
    if (activeConversationId === null) return;
    setSendError(null);
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

  function handleSelectAgent(agentId: string) {
    setSelectedAgentId(agentId);
    // Pre-fills the model selector with the agent's default, if it has one — the user can still
    // change it afterward, this is just a convenience.
    const agent = settings.agents.find((a) => a.id === agentId);
    if (agent?.providerId && isModel(settings, agent.providerId)) {
      setSelectedProviderId(agent.providerId);
    }
  }

  /** `persist` false only shows the message: a code project's turn saves the whole exchange itself (P103 b). */
  function appendMessage(conversationId: string, message: ChatMessage, titleSeed?: string, persist = true) {
    const agentId = selectedAgentId || undefined;
    const providerId = selectedProviderId || undefined;
    const projectId = currentProjectId || undefined;
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
    })
      .then((onDisk) => setConversations((prev) => replaceWithSaved(prev, onDisk)))
      .catch((err) => console.error("failed to persist conversation:", err));
  }

  async function handleSendMessage(content: string, attachments: Attachment[] = []) {
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
    if (activeConversationId === null) setActiveConversationId(conversationId);

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
        conversationId,
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

  /** The Stop button: the turn ends on its own, with what the engine had by then or an error to show. */
  function handleCancel() {
    if (activeConversationId === null) return;
    invoke("cancel_turn", { conversationId: activeConversationId }).catch((err) => setSendError(String(err)));
  }

  return (
    <div className={`app-shell${sidebarCollapsed ? " app-shell--sidebar-collapsed" : ""}`}>
      <Sidebar
        conversations={conversations}
        projects={projects}
        activeConversationId={activeConversationId}
        onSelectConversation={(id) => {
          setActiveConversationId(id);
          setView("chat");
        }}
        onNewConversation={() => {
          setActiveConversationId(null);
          setSelectedProjectId("");
          setView("chat");
        }}
        onOpenSettings={() => setView("settings")}
        onOpenUsage={() => setView("usage")}
        onOpenSync={() => setView("sync")}
        onOpenVault={() => setView("vault")}
        onOpenSkills={() => setView("skills")}
        onOpenProjects={() => setView("projects")}
        onOpenTasks={() => setView("tasks")}
        onOpenWorkspace={() => setView("workspace")}
        view={view}
        collapsed={sidebarCollapsed}
        onToggleCollapsed={handleToggleSidebarCollapsed}
      />
      {view === "settings" ? (
        <SettingsView />
      ) : view === "usage" ? (
        <UsageView />
      ) : view === "sync" ? (
        <SyncView />
      ) : view === "vault" ? (
        <VaultView />
      ) : view === "skills" ? (
        <SkillsView agents={settings.agents} providers={settings.providers} activeProvider={settings.activeProvider} />
      ) : view === "projects" ? (
        <ProjectsView onChanged={loadProjects} />
      ) : view === "tasks" ? (
        <TasksView agents={settings.agents} />
      ) : view === "workspace" ? (
        <WorkspaceView />
      ) : (
        <ChatArea
          activeConversation={activeConversation}
          onSendMessage={handleSendMessage}
          isSending={isSending}
          live={activeConversationId === null ? undefined : liveTurns[activeConversationId]}
          onCancel={handleCancel}
          sendError={sendError}
          agents={settings.agents}
          providers={settings.providers}
          combos={settings.combos}
          projects={projects}
          selectedProjectId={currentProjectId}
          onSelectProject={setSelectedProjectId}
          onMoveProject={(id) => void handleMoveProject(id)}
          selectedAgentId={selectedAgentId}
          selectedProviderId={selectedProviderId}
          onSelectAgent={handleSelectAgent}
          onSelectProvider={setSelectedProviderId}
          onOpenSettings={() => setView("settings")}
        />
      )}
      <ApprovalModal />
    </div>
  );
}

export default App;
