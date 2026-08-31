import { useEffect, useRef } from "react";
import type { Conversation } from "../types";
import { LogoMark } from "./Icons";
import MessageBubble from "./MessageBubble";
import MessageInput from "./MessageInput";

interface ChatAreaProps {
  activeConversation: Conversation | undefined;
  onSendMessage: (content: string) => void;
  isSending: boolean;
  sendError: string | null;
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

function ChatArea({ activeConversation, onSendMessage, isSending, sendError }: ChatAreaProps) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [activeConversation?.messages.length, isSending]);

  const hasMessages = !!activeConversation && activeConversation.messages.length > 0;

  return (
    <div className="chat-area">
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
