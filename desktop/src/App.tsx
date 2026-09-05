import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import ChatArea from "./components/ChatArea";
import Sidebar from "./components/Sidebar";
import SettingsView from "./components/SettingsView";
import UsageView from "./components/UsageView";
import type { Attachment, ChatMessage, Conversation, Settings, Usage } from "./types";

const emptySettings: Settings = {
  providers: [],
  activeProvider: "",
  vaultPath: "",
  tavilyKey: "",
  whisperKey: "",
  enableShell: false,
  defaultModels: {},
  mcpServers: [],
  agents: [],
};

function titleFromMessage(content: string): string {
  const collapsed = content.trim().replace(/\s+/g, " ");
  return collapsed.length > 40 ? `${collapsed.slice(0, 40)}…` : collapsed;
}

function App() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [view, setView] = useState<"chat" | "settings" | "usage">("chat");
  const [settings, setSettings] = useState<Settings>(emptySettings);
  const [selectedAgentId, setSelectedAgentId] = useState("");
  const [selectedProviderId, setSelectedProviderId] = useState("");

  const activeConversation = conversations.find((c) => c.id === activeConversationId);

  useEffect(() => {
    invoke<Conversation[]>("list_conversations")
      .then(setConversations)
      .catch((err) => console.error("failed to load conversation history:", err));
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
    setSelectedProviderId(settings.providers.some((p) => p.id === storedProviderId) ? storedProviderId : settings.activeProvider);
  }, [activeConversationId, settings]);

  function handleSelectAgent(agentId: string) {
    setSelectedAgentId(agentId);
    // Pre-fills the model selector with the agent's default, if it has one — the user can still
    // change it afterward, this is just a convenience.
    const agent = settings.agents.find((a) => a.id === agentId);
    if (agent?.providerId && settings.providers.some((p) => p.id === agent.providerId)) {
      setSelectedProviderId(agent.providerId);
    }
  }

  function appendMessage(conversationId: string, message: ChatMessage, titleSeed?: string) {
    setConversations((prev) => {
      const existing = prev.find((c) => c.id === conversationId);
      const agentId = selectedAgentId || undefined;
      const providerId = selectedProviderId || undefined;
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
          };

      void invoke("save_conversation", { conversation }).catch((err) =>
        console.error("failed to persist conversation:", err)
      );

      return existing ? prev.map((c) => (c.id === conversationId ? conversation : c)) : [conversation, ...prev];
    });
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

    appendMessage(conversationId, userMessage, content || "Image");
    if (activeConversationId === null) setActiveConversationId(conversationId);

    setSendError(null);
    setIsSending(true);
    try {
      const reply = await invoke<{ content: string; usage?: Usage }>("send_message", {
        history,
        content,
        attachments,
        agentId: selectedAgentId || null,
        // Only sent as an override when it actually differs from the active provider — the base
        // orchestrator already uses that one, no need to rebuild a `ModelProvider` for it.
        providerId: selectedProviderId && selectedProviderId !== settings.activeProvider ? selectedProviderId : null,
      });
      appendMessage(conversationId, {
        id: crypto.randomUUID(),
        role: "assistant",
        content: reply.content,
        createdAt: Date.now(),
        usage: reply.usage,
      });
    } catch (err) {
      setSendError(String(err));
    } finally {
      setIsSending(false);
    }
  }

  return (
    <div className="app-shell">
      <Sidebar
        conversations={conversations}
        activeConversationId={activeConversationId}
        onSelectConversation={(id) => {
          setActiveConversationId(id);
          setView("chat");
        }}
        onNewConversation={() => {
          setActiveConversationId(null);
          setView("chat");
        }}
        onOpenSettings={() => setView("settings")}
        onOpenUsage={() => setView("usage")}
        view={view}
      />
      {view === "settings" ? (
        <SettingsView />
      ) : view === "usage" ? (
        <UsageView />
      ) : (
        <ChatArea
          activeConversation={activeConversation}
          onSendMessage={handleSendMessage}
          isSending={isSending}
          sendError={sendError}
          agents={settings.agents}
          providers={settings.providers}
          selectedAgentId={selectedAgentId}
          selectedProviderId={selectedProviderId}
          onSelectAgent={handleSelectAgent}
          onSelectProvider={setSelectedProviderId}
        />
      )}
    </div>
  );
}

export default App;
