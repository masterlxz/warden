import { useState } from "react";
import "./App.css";
import ChatArea from "./components/ChatArea";
import Sidebar from "./components/Sidebar";
import type { ChatMessage, Conversation } from "./types";

function titleFromMessage(content: string): string {
  const collapsed = content.trim().replace(/\s+/g, " ");
  return collapsed.length > 40 ? `${collapsed.slice(0, 40)}…` : collapsed;
}

function App() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);

  const activeConversation = conversations.find((c) => c.id === activeConversationId);

  function handleSendMessage(content: string) {
    const message: ChatMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content,
      createdAt: Date.now(),
    };

    if (activeConversationId === null) {
      const conversation: Conversation = {
        id: crypto.randomUUID(),
        title: titleFromMessage(content),
        messages: [message],
        createdAt: message.createdAt,
        updatedAt: message.createdAt,
      };
      setConversations((prev) => [conversation, ...prev]);
      setActiveConversationId(conversation.id);
      return;
    }

    setConversations((prev) =>
      prev.map((conversation) =>
        conversation.id === activeConversationId
          ? { ...conversation, messages: [...conversation.messages, message], updatedAt: message.createdAt }
          : conversation
      )
    );
  }

  return (
    <div className="app-shell">
      <Sidebar
        conversations={conversations}
        activeConversationId={activeConversationId}
        onSelectConversation={setActiveConversationId}
        onNewConversation={() => setActiveConversationId(null)}
      />
      <ChatArea activeConversation={activeConversation} onSendMessage={handleSendMessage} />
    </div>
  );
}

export default App;
