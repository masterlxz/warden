import type { ChatMessage } from "../types";

interface MessageBubbleProps {
  message: ChatMessage;
}

function MessageBubble({ message }: MessageBubbleProps) {
  return (
    <div className={`message-bubble message-bubble--${message.role}`}>
      <p className="message-bubble-content">{message.content}</p>
    </div>
  );
}

export default MessageBubble;
