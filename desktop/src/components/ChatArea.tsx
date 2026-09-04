import { useEffect, useRef } from "react";
import type { AgentEntry, Attachment, Conversation, ProviderEntry } from "../types";
import { LogoMark } from "./Icons";
import MessageBubble from "./MessageBubble";
import MessageInput from "./MessageInput";

interface ChatAreaProps {
  activeConversation: Conversation | undefined;
  onSendMessage: (content: string, attachments: Attachment[]) => void;
  isSending: boolean;
  sendError: string | null;
  agents: AgentEntry[];
  providers: ProviderEntry[];
  selectedAgentId: string;
  selectedProviderId: string;
  onSelectAgent: (agentId: string) => void;
  onSelectProvider: (providerId: string) => void;
}

function ThinkingIndicator() {
  return (
    <div className="message-row message-row--assistant">
      <div className="message-avatar">
        <LogoMark size={18} />
      </div>
      <div className="thinking-indicator" aria-label="Warden is thinking">
        <span />
        <span />
        <span />
      </div>
    </div>
  );
}

function ChatArea({
  activeConversation,
  onSendMessage,
  isSending,
  sendError,
  agents,
  providers,
  selectedAgentId,
  selectedProviderId,
  onSelectAgent,
  onSelectProvider,
}: ChatAreaProps) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [activeConversation?.messages.length, isSending]);

  const hasMessages = !!activeConversation && activeConversation.messages.length > 0;

  return (
    <div className="chat-area">
      <div className="chat-header">
        <select
          className="chat-header-select"
          aria-label="Agent"
          value={selectedAgentId}
          onChange={(e) => onSelectAgent(e.currentTarget.value)}
        >
          <option value="">No agent</option>
          {agents.map((a) => (
            <option key={a.id} value={a.id}>
              {a.id}
            </option>
          ))}
        </select>
        <select
          className="chat-header-select"
          aria-label="Model"
          value={selectedProviderId}
          onChange={(e) => onSelectProvider(e.currentTarget.value)}
        >
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.id}
            </option>
          ))}
        </select>
      </div>
      <div className="chat-messages" role="log" aria-live="polite" aria-label="Conversation messages">
        {!hasMessages ? (
          <div className="chat-empty-state">
            <LogoMark size={40} />
            <h1>How can I help you today?</h1>
          </div>
        ) : (
          <div className="chat-messages-column">
            {activeConversation!.messages.map((message) => (
              <MessageBubble key={message.id} message={message} />
            ))}
            {isSending && <ThinkingIndicator />}
            <div ref={bottomRef} />
          </div>
        )}
      </div>
      {sendError && (
        <div className="chat-error-banner" role="alert">
          {sendError}
        </div>
      )}
      <MessageInput onSend={onSendMessage} focusKey={activeConversation?.id ?? null} disabled={isSending} />
    </div>
  );
}

export default ChatArea;
