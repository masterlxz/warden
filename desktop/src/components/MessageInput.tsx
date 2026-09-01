import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { AttachIcon, CloseIcon, SendIcon } from "./Icons";
import type { Attachment } from "../types";

interface MessageInputProps {
  onSend: (content: string, attachments: Attachment[]) => void;
  /** Bumped by the parent whenever the active conversation changes, so the composer refocuses. */
  focusKey?: string | null;
  disabled?: boolean;
}

function MessageInput({ onSend, focusKey, disabled }: MessageInputProps) {
  const [draft, setDraft] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textareaRef.current?.focus();
  }, [focusKey]);

  function submit() {
    const content = draft.trim();
    if ((content === "" && attachments.length === 0) || disabled) return;
    onSend(content, attachments);
    setDraft("");
    setAttachments([]);
  }

  async function handleAttach() {
    setAttachError(null);
    const selected = await open({
      multiple: true,
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
    });
    if (!selected) return;

    const paths = Array.isArray(selected) ? selected : [selected];
    for (const path of paths) {
      try {
        const attachment = await invoke<Attachment>("read_attachment", { path });
        setAttachments((prev) => [...prev, attachment]);
      } catch (err) {
        setAttachError(String(err));
      }
    }
  }

  function removeAttachment(index: number) {
    setAttachments((prev) => prev.filter((_, i) => i !== index));
  }

  return (
    <div className="chat-input-dock">
      {attachError && <p className="chat-attach-error" role="alert">{attachError}</p>}
      {attachments.length > 0 && (
        <div className="chat-attachment-preview">
          {attachments.map((attachment, index) => (
            <div className="chat-attachment-thumb" key={index}>
              <img src={`data:${attachment.mimeType};base64,${attachment.data}`} alt="" />
              <button
                type="button"
                className="chat-attachment-remove"
                aria-label="Remove attachment"
                onClick={() => removeAttachment(index)}
              >
                <CloseIcon size={11} />
              </button>
            </div>
          ))}
        </div>
      )}
      <form
        className="chat-input-form"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <button type="button" className="chat-attach-btn" aria-label="Attach image" onClick={handleAttach} disabled={disabled}>
          <AttachIcon size={17} />
        </button>
        <textarea
          ref={textareaRef}
          className="chat-input-textarea"
          aria-label="Message"
          placeholder="Message Warden…"
          value={draft}
          onChange={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          disabled={disabled}
          rows={1}
        />
        <button
          type="submit"
          className="chat-send-btn"
          aria-label="Send message"
          disabled={disabled || (draft.trim() === "" && attachments.length === 0)}
        >
          <SendIcon size={17} />
        </button>
      </form>
      <p className="chat-input-hint">Warden can make mistakes. Check important info.</p>
    </div>
  );
}

export default MessageInput;
