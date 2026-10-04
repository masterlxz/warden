import { useState } from "react";
import type { ConversationSummary } from "../protocol/messages";
import type { OkResponse } from "../background/popup_protocol";
import FolderPicker from "./FolderPicker";
import { folderLabel } from "./workdir";

interface Props {
  conversations: ConversationSummary[];
  activeConversationId: string | null;
  pendingIds: string[];
  agentIds: string[];
  agentId: string | null;
  /** P102 — the open conversation's working folder, or the one a new conversation will start in. */
  workdir: string | null;
}

/** P78 — which of this device's conversations the chat shows, plus new/rename/delete. A `<select>`
 * rather than a list: the side panel is narrow. Talks to the background directly, like `SkillsView`;
 * the result comes back as a `conversationsChanged` event. */
export default function ConversationBar({ conversations, activeConversationId, pendingIds, agentIds, agentId, workdir }: Props) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [pickingFolder, setPickingFolder] = useState(false);
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
    setPickingFolder(false);
    setRenaming(null);
    setConfirmDelete(false);
    setError(null);
    void chrome.runtime.sendMessage({ type: "selectConversation", conversationId });
  }

  function startNew() {
    setPickingFolder(false);
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
      {(agentIds.length > 0 || agentId !== null) && (
        // P87 — which configured agent the next turns speak as; locked while an answer is on the way.
        <label className="conversation-bar-agent">
          Agente
          <select
            value={agentId ?? ""}
            disabled={activeConversationId !== null && pendingIds.includes(activeConversationId)}
            onFocus={() => void chrome.runtime.sendMessage({ type: "refreshAgents" })}
            onChange={(e) => void chrome.runtime.sendMessage({ type: "selectAgent", agentId: e.target.value || null })}
          >
            <option value="">Nenhum</option>
            {agentIds.map((id) => (
              <option key={id} value={id}>
                {id}
              </option>
            ))}
            {agentId !== null && !agentIds.includes(agentId) && <option value={agentId}>{agentId} (removido)</option>}
          </select>
        </label>
      )}
      {/* P102 — the working folder: picked before the first message, only shown after it. */}
      {(active ? workdir !== null : true) && (
        <span className="conversation-bar-agent">
          Pasta
          {active ? (
            <span className="folder-chip" title={workdir ?? ""}>
              {folderLabel(workdir ?? "")}
            </span>
          ) : (
            <>
              <button type="button" className="link-button folder-chip" onClick={() => setPickingFolder(true)} title={workdir ?? "Escolher uma pasta do hub para a IA trabalhar"}>
                {workdir !== null ? folderLabel(workdir) : "Nenhuma"}
              </button>
              {workdir !== null && (
                <button type="button" className="link-button" aria-label="Tirar a pasta" onClick={() => void run({ type: "selectWorkdir", path: null })}>
                  ×
                </button>
              )}
            </>
          )}
        </span>
      )}
      {pickingFolder && !active && (
        <FolderPicker
          initialPath={workdir}
          onCancel={() => setPickingFolder(false)}
          onPick={(path) => {
            setPickingFolder(false);
            void run({ type: "selectWorkdir", path });
          }}
        />
      )}
      {error && <p className="error-banner">{error}</p>}
    </div>
  );
}
