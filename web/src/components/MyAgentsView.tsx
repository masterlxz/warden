import { useCallback, useEffect, useState } from "react";
import type { ServerConnection } from "../hub/connection";
import type { AgentSettings } from "../hub/messages";

// A member's agents (P84 fatia 2): their own — which only they see, with the tools they have — and
// the ones the owner shared with them, which they can use but not change. The owner edits theirs in
// Settings; a member never gets there.

interface Draft {
  /** The name it had when opened; absent for a new one. */
  originalId?: string;
  id: string;
  persona: string;
  /** `null`: every tool they have. */
  allowedTools: string[] | null;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function toAgent(draft: Draft): AgentSettings {
  return {
    id: draft.id.trim(),
    persona: draft.persona,
    providerId: "",
    canDelegateToAgents: false,
    canManageAgents: false,
    canMessageAgents: false,
    canManageTasks: false,
    // O agente de um membro faz o que sempre fez: os workers e o segundo plano fazem parte das ferramentas dele.
    canStartTasks: true,
    canCreateWorkers: true,
    allowedTools: draft.allowedTools,
    autonomy: 4,
    approvalRequired: [],
  };
}

export default function MyAgentsView({ conn, onChanged }: { conn: ServerConnection | null; onChanged: () => void }) {
  const [agents, setAgents] = useState<AgentSettings[] | null>(null);
  const [tools, setTools] = useState<string[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const apply = useCallback((settings: { agents: AgentSettings[]; toolNames: string[] }) => {
    setAgents(settings.agents);
    setTools(settings.toolNames);
  }, []);

  useEffect(() => {
    conn
      ?.requestSettings()
      .then(({ settings }) => apply(settings))
      .catch((err) => setError(message(err)));
  }, [conn, apply]);

  async function save() {
    if (!conn || !draft) return;
    setBusy(true);
    setError(null);
    try {
      apply((await conn.saveOwnAgent(toAgent(draft), draft.originalId)).settings);
      setDraft(null);
      onChanged();
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(id: string) {
    if (!conn) return;
    setBusy(true);
    setError(null);
    try {
      apply((await conn.deleteOwnAgent(id)).settings);
      setConfirmDelete(null);
      onChanged();
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  }

  const mine = (agents ?? []).filter((a) => a.owner);
  const shared = (agents ?? []).filter((a) => !a.owner);

  return (
    <div className="skills-view">
      <div className="skills-header">
        <h2>Agentes</h2>
        <button type="button" className="primary-button" disabled={!conn || draft !== null} onClick={() => setDraft({ id: "", persona: "", allowedTools: null })}>
          Criar agente
        </button>
      </div>
      <p className="skills-hint">
        Os seus agentes só você vê. Eles usam a sua memória e, no máximo, as ferramentas que você tem. Os compartilhados com você aparecem no chat
        também, mas só quem os criou muda.
      </p>
      {error && <p className="error-banner">{error}</p>}

      {draft && (
        <form
          className="settings-confirm skills-editor"
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <label>
            Nome
            <input value={draft.id} onChange={(e) => setDraft({ ...draft, id: e.target.value })} placeholder="cozinheiro" autoFocus />
          </label>
          <label>
            Persona
            <textarea rows={6} value={draft.persona} onChange={(e) => setDraft({ ...draft, persona: e.target.value })} placeholder="Quem ele é, no que é bom e como deve se comportar." />
          </label>
          <label className="settings-check">
            <input type="checkbox" checked={draft.allowedTools !== null} onChange={(e) => setDraft({ ...draft, allowedTools: e.target.checked ? [] : null })} />
            Limitar as ferramentas
          </label>
          {draft.allowedTools !== null && (
            <fieldset className="settings-tools">
              {tools.map((tool) => (
                <label key={tool} className="settings-check">
                  <input
                    type="checkbox"
                    checked={draft.allowedTools!.includes(tool)}
                    onChange={(e) => setDraft({ ...draft, allowedTools: e.target.checked ? [...draft.allowedTools!, tool] : draft.allowedTools!.filter((t) => t !== tool) })}
                  />
                  <code>{tool}</code>
                </label>
              ))}
            </fieldset>
          )}
          <div className="skills-actions">
            <button type="submit" className="primary-button" disabled={busy || !conn || draft.id.trim() === "" || draft.persona.trim() === ""}>
              {busy ? "Aguarde…" : "Salvar"}
            </button>
            <button type="button" className="link-button" disabled={busy} onClick={() => setDraft(null)}>
              Cancelar
            </button>
          </div>
        </form>
      )}

      {agents === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : (
        <>
          <h3 className="settings-section-title">Seus</h3>
          {mine.length === 0 ? (
            <p className="skills-hint">Nenhum ainda.</p>
          ) : (
            <ul className="skills-list">
              {mine.map((agent) => (
                <li key={agent.id} className="skills-item">
                  <div className="skills-item-header">
                    <span className="skills-item-name">{agent.id}</span>
                  </div>
                  <p className="skills-item-description">{agent.persona}</p>
                  <p className="skills-hint">{agent.allowedTools === null ? "Todas as suas ferramentas" : `Só: ${agent.allowedTools.join(", ") || "nenhuma"}`}</p>
                  {confirmDelete === agent.id ? (
                    <div className="skills-actions">
                      <button type="button" className="primary-button skills-danger" disabled={busy} onClick={() => void remove(agent.id)}>
                        Apagar de vez
                      </button>
                      <button type="button" className="link-button" onClick={() => setConfirmDelete(null)}>
                        Cancelar
                      </button>
                    </div>
                  ) : (
                    <div className="skills-actions">
                      <button
                        type="button"
                        className="link-button"
                        disabled={draft !== null}
                        onClick={() => setDraft({ originalId: agent.id, id: agent.id, persona: agent.persona, allowedTools: agent.allowedTools })}
                      >
                        Editar
                      </button>
                      <button type="button" className="link-button skills-danger" onClick={() => setConfirmDelete(agent.id)}>
                        Apagar
                      </button>
                    </div>
                  )}
                </li>
              ))}
            </ul>
          )}
          <h3 className="settings-section-title">Compartilhados com você</h3>
          {shared.length === 0 ? (
            <p className="skills-hint">Nenhum.</p>
          ) : (
            <ul className="skills-list">
              {shared.map((agent) => (
                <li key={agent.id} className="skills-item">
                  <span className="skills-item-name">{agent.id}</span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}
