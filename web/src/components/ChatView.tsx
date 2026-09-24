import { useEffect, useRef, useState, type AnchorHTMLAttributes } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { ChatEntry } from "../hub/connection";
import type { Attachment } from "../hub/messages";

interface Props {
  entries: ChatEntry[];
  pending: boolean;
  /** The connection is down — typing is fine, sending isn't. */
  disabled: boolean;
  onSend: (message: string) => void;
}

/** Links from the model open in a new tab, never replacing the chat. */
function ExternalLink({ children, ...props }: AnchorHTMLAttributes<HTMLAnchorElement>) {
  return (
    <a {...props} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

function AttachmentView({ attachment }: { attachment: Attachment }) {
  const src = `data:${attachment.mimeType};base64,${attachment.data}`;
  if (attachment.mimeType.startsWith("image/")) return <img className="attachment-image" src={src} alt="" />;
  if (attachment.mimeType.startsWith("audio/")) return <audio className="attachment-audio" src={src} controls />;
  return (
    <a className="attachment-file" href={src} download>
      Baixar anexo ({attachment.mimeType})
    </a>
  );
}

export default function ChatView({ entries, pending, disabled, onSend }: Props) {
  const [draft, setDraft] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [entries, pending]);

  function submit() {
    const trimmed = draft.trim();
    if (!trimmed || pending || disabled) return;
    onSend(trimmed);
    setDraft("");
  }

  return (
    <div className="chat">
      <div className="chat-scroll" aria-live="polite">
        {entries.length === 0 && !pending ? (
          <p className="chat-empty">Nenhuma mensagem ainda. Esta conversa é a deste navegador no hub.</p>
        ) : (
          <ul className="chat-list">
            {entries.map((entry, i) => (
              <li key={i} className={`bubble bubble--${entry.role}`}>
                {entry.role === "user" ? (
                  <p className="bubble-plain">{entry.content}</p>
                ) : (
                  <div className="bubble-markdown">
                    <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ExternalLink }}>
                      {entry.content}
                    </ReactMarkdown>
                  </div>
                )}
                {entry.attachments.map((attachment, j) => (
                  <AttachmentView key={j} attachment={attachment} />
                ))}
              </li>
            ))}
            {pending && (
              <li className="bubble bubble--assistant bubble--pending" aria-label="Pensando">
                <span className="dot" />
                <span className="dot" />
                <span className="dot" />
              </li>
            )}
          </ul>
        )}
        <div ref={endRef} />
      </div>

      <form
        className="composer"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder="Mensagem (Shift+Enter quebra a linha)"
          rows={1}
        />
        <button type="submit" className="primary-button" disabled={pending || disabled || !draft.trim()}>
          Enviar
        </button>
      </form>
    </div>
  );
}
