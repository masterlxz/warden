import { useEffect, useRef, useState, type AnchorHTMLAttributes } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { AttachmentError, checkLimits, composeMessage, mediaOf, prepareFile, type PendingAttachment } from "../hub/attachments";
import type { ChatEntry } from "../hub/connection";
import type { Attachment } from "../hub/messages";
import { canRecord, VoiceRecorder } from "../hub/recorder";

interface Props {
  entries: ChatEntry[];
  pending: boolean;
  /** The connection is down — typing is fine, sending isn't. */
  disabled: boolean;
  onSend: (message: string, attachments: Attachment[]) => void;
  /** The hub's transcription of a voice recording (P78). */
  onTranscribe: (audio: Attachment) => Promise<string>;
}

type Voice = { kind: "idle" } | { kind: "recording"; recorder: VoiceRecorder } | { kind: "transcribing" };

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
    <a className="attachment-file" href={src} download={attachment.mimeType === "application/pdf" ? "anexo.pdf" : undefined}>
      {attachment.mimeType === "application/pdf" ? "📄 PDF anexado" : `Baixar anexo (${attachment.mimeType})`}
    </a>
  );
}

/** One thing waiting in the composer: a thumbnail for images, the name for the rest. */
function PendingChip({ item, onRemove }: { item: PendingAttachment; onRemove: () => void }) {
  const isImage = item.kind === "media" && item.attachment.mimeType.startsWith("image/");
  return (
    <li className="pending-chip">
      {isImage ? (
        <img src={`data:${item.attachment.mimeType};base64,${item.attachment.data}`} alt="" />
      ) : (
        <span className="pending-chip-icon" aria-hidden="true">
          {item.kind === "text" ? "📝" : "📄"}
        </span>
      )}
      <span className="pending-chip-name">{item.name}</span>
      <button type="button" className="pending-chip-remove" onClick={onRemove} aria-label={`Remover ${item.name}`}>
        ×
      </button>
    </li>
  );
}

export default function ChatView({ entries, pending, disabled, onSend, onTranscribe }: Props) {
  const [draft, setDraft] = useState("");
  const [attached, setAttached] = useState<PendingAttachment[]>([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [voice, setVoice] = useState<Voice>({ kind: "idle" });
  const [dragging, setDragging] = useState(false);
  const endRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const microphone = canRecord();

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [entries, pending]);

  // A recording still going when the chat goes away would keep the microphone on.
  const recordingRef = useRef<VoiceRecorder | null>(null);
  recordingRef.current = voice.kind === "recording" ? voice.recorder : null;
  useEffect(() => () => recordingRef.current?.cancel(), []);

  async function addFiles(files: File[]) {
    if (files.length === 0) return;
    setAttachError(null);
    setPreparing(true);
    const prepared: PendingAttachment[] = [];
    const errors: string[] = [];
    for (const file of files) {
      try {
        prepared.push(await prepareFile(file));
      } catch (err) {
        errors.push(err instanceof AttachmentError ? err.message : `${file.name}: ${err instanceof Error ? err.message : String(err)}`);
      }
    }
    setPreparing(false);
    setAttached((current) => [...current, ...prepared]);
    if (errors.length > 0) setAttachError(errors.join(" · "));
  }

  const limitError = checkLimits(attached);
  const hasContent = draft.trim() !== "" || attached.length > 0;
  const canSend = hasContent && !pending && !disabled && !preparing && limitError === null && voice.kind === "idle";

  function submit() {
    if (!canSend) return;
    onSend(composeMessage(draft.trim(), attached), mediaOf(attached));
    setDraft("");
    setAttached([]);
    setAttachError(null);
  }

  async function toggleVoice() {
    setAttachError(null);
    if (voice.kind === "idle") {
      try {
        setVoice({ kind: "recording", recorder: await VoiceRecorder.start() });
      } catch (err) {
        setAttachError(`Não foi possível usar o microfone: ${err instanceof Error ? err.message : String(err)}`);
      }
      return;
    }
    if (voice.kind !== "recording") return;
    setVoice({ kind: "transcribing" });
    try {
      const text = (await onTranscribe(await voice.recorder.stop())).trim();
      if (text) setDraft((current) => (current.trim() ? `${current.trimEnd()} ${text}` : text));
      textareaRef.current?.focus();
    } catch (err) {
      setAttachError(`Não foi possível transcrever: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setVoice({ kind: "idle" });
    }
  }

  const micTitle = !microphone
    ? "O microfone só funciona com o hub em HTTPS (Tailscale) ou em localhost"
    : voice.kind === "recording"
      ? "Parar e transcrever"
      : voice.kind === "transcribing"
        ? "Transcrevendo…"
        : "Ditar por voz";

  return (
    <div
      className={dragging ? "chat chat--dragging" : "chat"}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes("Files")) return;
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDragging(false);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        void addFiles([...e.dataTransfer.files]);
      }}
    >
      <div className="chat-scroll" aria-live="polite">
        {entries.length === 0 && !pending ? (
          <p className="chat-empty">Nenhuma mensagem ainda. A conversa é criada no hub quando você manda a primeira.</p>
        ) : (
          <ul className="chat-list">
            {entries.map((entry, i) => (
              <li key={i} className={`bubble bubble--${entry.role}`}>
                {entry.content !== "" &&
                  (entry.role === "user" ? (
                    <p className="bubble-plain">{entry.content}</p>
                  ) : (
                    <div className="bubble-markdown">
                      <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ExternalLink }}>
                        {entry.content}
                      </ReactMarkdown>
                    </div>
                  ))}
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
        {(attached.length > 0 || preparing || attachError || limitError) && (
          <div className="composer-attachments">
            {attached.length > 0 && (
              <ul className="pending-chips">
                {attached.map((item, i) => (
                  <PendingChip key={i} item={item} onRemove={() => setAttached((current) => current.filter((_, j) => j !== i))} />
                ))}
              </ul>
            )}
            {preparing && <p className="composer-note">Preparando anexos…</p>}
            {(attachError ?? limitError) && <p className="composer-error">{attachError ?? limitError}</p>}
          </div>
        )}
        <div className="composer-row">
          <input
            ref={fileInputRef}
            type="file"
            multiple
            hidden
            accept="image/png,image/jpeg,image/webp,image/gif,application/pdf,text/*,.md,.csv,.json,.yaml,.yml,.toml,.log"
            onChange={(e) => {
              void addFiles([...(e.target.files ?? [])]);
              e.target.value = "";
            }}
          />
          <button type="button" className="icon-button" onClick={() => fileInputRef.current?.click()} title="Anexar imagem, PDF ou arquivo de texto" aria-label="Anexar">
            📎
          </button>
          <button
            type="button"
            className={voice.kind === "recording" ? "icon-button icon-button--recording" : "icon-button"}
            onClick={() => void toggleVoice()}
            disabled={!microphone || voice.kind === "transcribing" || disabled}
            title={micTitle}
            aria-label={micTitle}
          >
            {voice.kind === "transcribing" ? "…" : voice.kind === "recording" ? "■" : "🎤"}
          </button>
          <textarea
            ref={textareaRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onPaste={(e) => {
              const files = [...e.clipboardData.files];
              if (files.length === 0) return;
              e.preventDefault();
              void addFiles(files);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                submit();
              }
            }}
            placeholder={voice.kind === "recording" ? "Gravando… toque em ■ para parar" : "Mensagem (Shift+Enter quebra a linha)"}
            rows={1}
          />
          <button type="submit" className="primary-button" disabled={!canSend}>
            Enviar
          </button>
        </div>
      </form>
    </div>
  );
}
