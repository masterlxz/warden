import { useEffect, useRef, useState } from "react";

interface MessageInputProps {
  onSend: (content: string) => void;
  /** Bumped by the parent whenever the active conversation changes, so the composer refocuses. */
  focusKey?: string | null;
}

function MessageInput({ onSend, focusKey }: MessageInputProps) {
  const [draft, setDraft] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textareaRef.current?.focus();
  }, [focusKey]);

  function submit() {
    const content = draft.trim();
    if (content === "") return;
    onSend(content);
    setDraft("");
  }

  return (
    <form
      className="chat-input-form"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <textarea
        ref={textareaRef}
        className="chat-input-textarea"
        aria-label="Message"
        placeholder="Type a message…"
        value={draft}
        onChange={(e) => setDraft(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            submit();
          }
        }}
        rows={1}
      />
      <button type="submit" className="chat-send-btn" aria-label="Send message" disabled={draft.trim() === ""}>
        Send
      </button>
    </form>
  );
}

export default MessageInput;
