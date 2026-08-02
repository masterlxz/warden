import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import ChatArea from "./components/ChatArea";
import Sidebar from "./components/Sidebar";
import SettingsView from "./components/SettingsView";
import type { ChatMessage, Conversation } from "./types";

function titleFromMessage(content: string): string {
  const collapsed = content.trim().replace(/\s+/g, " ");
  return collapsed.length > 40 ? `${collapsed.slice(0, 40)}…` : collapsed;
}

function App() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [view, setView] = useState<"chat" | "settings">("chat");

  const activeConversation = conversations.find((c) => c.id === activeConversationId);

  function appendMessage(conversationId: string, message: ChatMessage, titleSeed?: string) {
    setConversations((prev) => {
      if (!prev.some((c) => c.id === conversationId)) {
        const conversation: Conversation = {
          id: conversationId,
          title: titleFromMessage(titleSeed ?? message.content),
          messages: [message],
          createdAt: message.createdAt,
          updatedAt: message.createdAt,
        };
        return [conversation, ...prev];
      }
      return prev.map((c) =>
        c.id === conversationId ? { ...c, messages: [...c.messages, message], updatedAt: message.createdAt } : c
      );
    });
  }

  async function handleSendMessage(content: string) {
    const conversationId = activeConversationId ?? crypto.randomUUID();
    const history = (activeConversation?.messages ?? []).map(({ role, content }) => ({ role, content }));
    const userMessage: ChatMessage = { id: crypto.randomUUID(), role: "user", content, createdAt: Date.now() };

    appendMessage(conversationId, userMessage, content);
    if (activeConversationId === null) setActiveConversationId(conversationId);

    setSendError(null);
    setIsSending(true);
    try {
      const reply = await invoke<string>("send_message", { history, content });
      appendMessage(conversationId, {
        id: crypto.randomUUID(),
        role: "assistant",
        content: reply,
        createdAt: Date.now(),
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
      />
      {view === "settings" ? (
        <SettingsView />
      ) : (
        <ChatArea
          activeConversation={activeConversation}
          onSendMessage={handleSendMessage}
          isSending={isSending}
          sendError={sendError}
        />
      )}
    </div>
  );
}

export default App;
