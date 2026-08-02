import { useEffect, useRef } from "react";
import type { Conversation } from "../types";
import MessageBubble from "./MessageBubble";
import MessageInput from "./MessageInput";

interface ChatAreaProps {
  activeConversation: Conversation | undefined;
  onSendMessage: (content: string) => void;
}

function ChatArea({ activeConversation, onSendMessage }: ChatAreaProps) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [activeConversation?.messages.length]);

  return (
    <div className="chat-area">
      <div className="chat-messages" role="log" aria-live="polite" aria-label="Conversation messages">
        {!activeConversation || activeConversation.messages.length === 0 ? (
          <div className="chat-empty-state">
            <p>Say something to start a conversation.</p>
          </div>
        ) : (
          <>
            {activeConversation.messages.map((message) => (
              <MessageBubble key={message.id} message={message} />
            ))}
            <div ref={bottomRef} />
          </>
        )}
      </div>
      <MessageInput onSend={onSendMessage} focusKey={activeConversation?.id ?? null} />
    </div>
  );
}

export default ChatArea;
