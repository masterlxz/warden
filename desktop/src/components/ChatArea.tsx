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
  onOpenSettings: () => void;
}

function personaPreview(persona: string): string {
  const collapsed = persona.trim().replace(/\s+/g, " ");
  return collapsed.length > 80 ? `${collapsed.slice(0, 80)}…` : collapsed;
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
  onOpenSettings,
}: ChatAreaProps) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [activeConversation?.messages.length, isSending]);

  const hasMessages = !!activeConversation && activeConversation.messages.length > 0;
  // A conversation's agent is chosen once, before its first message, then locked for good (P45)
  // — this is the "not chosen yet" gate: no messages persisted yet, and no agent picked yet
  // either (picking one doesn't send a message by itself, see onSelectAgent below).
  const needsAgentPick = !hasMessages && !selectedAgentId;

  return (
    <div className="chat-area">
      <div className="chat-header">
        {needsAgentPick ? (
          <span className="chat-header-label chat-header-label--muted">Pick an agent to start</span>
        ) : (
          <span className="chat-header-label" title="The agent driving this conversation — locked once chosen">
            {selectedAgentId || "No agent"}
          </span>
        )}
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
        {needsAgentPick ? (
          <div className="agent-picker">
            {agents.length === 0 ? (
              <>
                <LogoMark size={40} />
                <h1>No agents yet</h1>
                <p>Create one in Settings to start a conversation.</p>
                <button type="button" className="agent-picker-settings-btn" onClick={onOpenSettings}>
                  Open Settings
                </button>
              </>
            ) : (
              <>
                <h1>Which agent should drive this conversation?</h1>
                <div className="agent-picker-list">
                  {agents.map((a) => (
                    <button
                      key={a.id}
                      type="button"
                      className="agent-picker-card"
                      onClick={() => onSelectAgent(a.id)}
                    >
                      <span className="agent-picker-card-name">{a.id}</span>
                      {a.persona && <span className="agent-picker-card-persona">{personaPreview(a.persona)}</span>}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        ) : !hasMessages ? (
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
      <MessageInput
        onSend={onSendMessage}
        focusKey={activeConversation?.id ?? null}
        disabled={isSending || needsAgentPick}
      />
    </div>
  );
}

export default ChatArea;
