import { useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { ChatEntry } from "../background/connection";
import { PopupMarkdownLink } from "./MarkdownLink";
import { repliesLabel, type ThreadInfo } from "../protocol/threads";

interface Props {
  serverName: string;
  history: ChatEntry[];
  pending: boolean;
  /** P125 — the threads of the open conversation, by the id of the message they hang from. */
  threads: Record<string, ThreadInfo>;
  /** P125 — the open conversation is itself a thread: no thread inside a thread. */
  inThread: boolean;
  onOpenThread: (messageId: string) => void;
  onSend: (message: string) => void;
  onDisconnect: () => void;
}

export default function ChatView({ serverName, history, pending, threads, inThread, onOpenThread, onSend, onDisconnect }: Props) {
  const [draft, setDraft] = useState("");

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = draft.trim();
    if (!trimmed || pending) return;
    onSend(trimmed);
    setDraft("");
  }

  return (
    <div className="chat-view">
      <header className="chat-header">
        <span>Conectado a {serverName}</span>
        <button type="button" className="link-button" onClick={onDisconnect}>
          Desconectar
        </button>
      </header>

      <ul className="chat-history" aria-live="polite">
        {history.map((entry, i) => (
          <li key={i} className={`chat-entry chat-entry--${entry.role}`}>
            <div className="chat-entry-content">
              <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: PopupMarkdownLink }}>
                {entry.content}
              </ReactMarkdown>
            </div>
            {!inThread && entry.id && entry.role !== "error" && (
              // P125 — a message the hub already has an id for can start a thread, or show the one it has.
              <div className="chat-entry-thread">
                {threads[entry.id] && threads[entry.id].replies > 0 ? (
                  <button type="button" className="link-button" onClick={() => onOpenThread(entry.id!)}>
                    {repliesLabel(threads[entry.id].replies)}
                  </button>
                ) : (
                  <button type="button" className="link-button" onClick={() => onOpenThread(entry.id!)}>
                    Responder em thread
                  </button>
                )}
              </div>
            )}
          </li>
        ))}
        {pending && <li className="chat-entry chat-entry--pending">…</li>}
      </ul>

      <form className="chat-composer" onSubmit={handleSubmit}>
        <input value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="Mensagem" disabled={pending} />
        <button type="submit" disabled={pending || !draft.trim()}>
          Enviar
        </button>
      </form>
    </div>
  );
}
