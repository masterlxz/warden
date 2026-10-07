import { useEffect, useRef, useState } from "react";
import type { Attachment, ChatMessage } from "../types";
import { hubChat, hubHistory } from "../lib/hub";
import { decorateLastAnswer, HubTurnError } from "../lib/hubMap";
import MessageBubble from "./MessageBubble";
import MessageInput from "./MessageInput";

/** P125 — the thread of a message, on a hub: a chat of its own beside the conversation, tied to a child conversation of it. The model
 * sees the conversation up to the message the thread came from and then only what is said here; nothing goes back to the main
 * conversation but the counter of replies on the message. `threadId` is the thread's conversation (the one it already has, or a new id
 * the hub creates with the first reply) and `parent` is the message it comes from, said only with the first. */
function ThreadPanel({
  threadId,
  parent,
  anchor,
  agentId,
  ready,
  onClose,
  onChanged,
}: {
  threadId: string;
  parent: { conversationId: string; messageId: string };
  /** The message the thread came from, shown at the top. */
  anchor: ChatMessage;
  /** The agent of the main conversation: the thread talks to the same one. */
  agentId: string;
  /** The hub is connected. */
  ready: boolean;
  onClose: () => void;
  /** A reply was saved: the list of conversations has a new counter to read. */
  onChanged: () => void;
}) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  // The thread's history, when it already exists (one that doesn't comes back empty).
  useEffect(() => {
    let alive = true;
    setMessages([]);
    setError(null);
    hubHistory(threadId).then(
      (saved) => alive && setMessages(saved),
      (err) => alive && setError(`Could not load the thread: ${String(err)}`),
    );
    return () => {
      alive = false;
    };
  }, [threadId]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [messages.length, sending]);

  async function send(content: string, attachments: Attachment[]) {
    // The hub has this thread once it saved a message in it; until then the turn is the one that creates it.
    const creating = !messages.some((m) => m.hubId);
    setError(null);
    setSending(true);
    setMessages((prev) => [...prev, { id: crypto.randomUUID(), role: "user", content, createdAt: Date.now(), ...(attachments.length > 0 ? { attachments } : {}) }]);
    try {
      const reply = await hubChat({ content, attachments, conversationId: threadId, agentId, projectId: "", workdir: "", creating, threadOf: parent });
      setMessages(decorateLastAnswer(await hubHistory(threadId), reply));
    } catch (err) {
      setError(err instanceof HubTurnError ? err.message : String(err));
    } finally {
      setSending(false);
      onChanged();
    }
  }

  return (
    <aside className="thread-panel" aria-label="Thread">
      <div className="thread-header">
        <strong>Thread</strong>
        <button type="button" className="settings-browse-btn" onClick={onClose} aria-label="Close the thread">
          Close
        </button>
      </div>
      <blockquote className="thread-anchor">
        <span className="settings-hint">{anchor.role === "user" ? "You" : "Assistant"} said:</span>
        <p>{anchor.content.length > 400 ? `${anchor.content.slice(0, 400)}…` : anchor.content}</p>
      </blockquote>
      <div className="thread-messages">
        {messages.map((message) => (
          <MessageBubble key={message.id} message={message} />
        ))}
        {sending && <p className="settings-hint">Thinking…</p>}
        {error && (
          <p className="chat-attach-error" role="alert">
            {error}
          </p>
        )}
        <div ref={bottomRef} />
      </div>
      <MessageInput onSend={(content, attachments) => void send(content, attachments)} focusKey={threadId} disabled={sending || !ready} />
    </aside>
  );
}

export default ThreadPanel;
