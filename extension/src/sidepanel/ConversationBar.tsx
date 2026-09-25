import { useState } from "react";
import type { ConversationSummary } from "../protocol/messages";
import type { OkResponse } from "../background/popup_protocol";

interface Props {
  conversations: ConversationSummary[];
  activeConversationId: string | null;
  pendingIds: string[];
}

/** P78 — which of this device's conversations the chat shows, plus new/rename/delete. A `<select>`
 * rather than a list: the side panel is narrow. Talks to the background directly, like `SkillsView`;
 * the result comes back as a `conversationsChanged` event. */
export default function ConversationBar({ conversations, activeConversationId, pendingIds }: Props) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const active = conversations.find((c) => c.id === activeConversationId);
  // Nothing on the hub to rename or delete yet, or its answer is still on the way.
  const locked = busy || !active || pendingIds.includes(active.id);

  async function run(request: object) {
    setBusy(true);
    setError(null);
    const res = (await chrome.runtime.sendMessage(request)) as OkResponse;
    setBusy(false);
    if (!res.ok) setError(res.error ?? "falhou");
    return res.ok;
  }

  function select(conversationId: string) {
    setRenaming(null);
    setConfirmDelete(false);
    setError(null);
    void chrome.runtime.sendMessage({ type: "selectConversation", conversationId });
  }

  function startNew() {
    setRenaming(null);
    setConfirmDelete(false);
    setError(null);
    void chrome.runtime.sendMessage({ type: "newConversation" });
  }

  async function saveRename(e: React.FormEvent) {
    e.preventDefault();
    if (!active || renaming === null) return;
    const title = renaming.trim();
    if (title && title !== active.title && !(await run({ type: "renameConversation", conversationId: active.id, title }))) return;
    setRenaming(null);
  }

  async function remove() {
    if (!active) return;
    if (await run({ type: "deleteConversation", conversationId: active.id })) setConfirmDelete(false);
  }

  if (renaming !== null) {
    return (
      <form className="conversation-bar" onSubmit={saveRename}>
        <input autoFocus value={renaming} maxLength={120} aria-label="Novo título" disabled={busy} onChange={(e) => setRenaming(e.target.value)} />
        <button type="submit" disabled={busy || !renaming.trim()}>
          Salvar
        </button>
        <button type="button" className="link-button" onClick={() => setRenaming(null)}>
          Cancelar
        </button>
        {error && <p className="error-banner">{error}</p>}
      </form>
    );
  }

  return (
    <div className="conversation-bar">
      <select aria-label="Conversa" value={active ? active.id : ""} onChange={(e) => select(e.target.value)}>
        {!active && <option value="">Nova conversa</option>}
        {conversations.map((c) => (
          <option key={c.id} value={c.id}>
            {pendingIds.includes(c.id) ? `${c.title} (respondendo…)` : c.title || "Sem título"}
          </option>
        ))}
      </select>
      <span className="conversation-bar-actions">
        {confirmDelete ? (
          <>
            <button type="button" className="link-button skills-danger" disabled={locked} onClick={() => void remove()}>
              Apagar mesmo
            </button>
            <button type="button" className="link-button" onClick={() => setConfirmDelete(false)}>
              Manter
            </button>
          </>
        ) : (
          <>
            <button type="button" className="link-button" onClick={startNew}>
              Nova
            </button>
            <button type="button" className="link-button" disabled={locked} onClick={() => active && setRenaming(active.title)}>
              Renomear
            </button>
            <button type="button" className="link-button" disabled={locked} onClick={() => setConfirmDelete(true)}>
              Apagar
            </button>
          </>
        )}
      </span>
      {error && <p className="error-banner">{error}</p>}
    </div>
  );
}
