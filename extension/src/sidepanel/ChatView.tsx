import { useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { ChatEntry } from "../background/connection";
import { PopupMarkdownLink } from "./MarkdownLink";

interface Props {
  serverName: string;
  history: ChatEntry[];
  pending: boolean;
  onSend: (message: string) => void;
  onDisconnect: () => void;
}

export default function ChatView({ serverName, history, pending, onSend, onDisconnect }: Props) {
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
