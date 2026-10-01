import { useCallback, useEffect, useState } from "react";
import type { ServerConnection } from "../hub/connection";
import type { SkillDto, UserInfo } from "../hub/messages";

// Adapted from the extension's `sidepanel/SkillsView.tsx` (P78), talking to the hub directly
// instead of through a background service worker.

/** What the editor form is doing: `new` (name editable, refuses a taken name) or `edit` (name
 * locked, overwrites) — same split as the desktop's SkillsView. */
interface EditorState {
  mode: "new" | "edit";
  skill: SkillDto;
  /** A suggestion being edited: whether saving accepts it (P104). */
  accept?: boolean;
}

const emptySkill: SkillDto = { name: "", description: "", body: "", agents: [] };

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SkillsView({
  conn,
  user,
  onLearningChange,
}: {
  conn: ServerConnection | null;
  /** Set for a member: their own learning switch shows when the workspace has learning on. */
  user?: UserInfo;
  onLearningChange?: (optOut: boolean) => void;
}) {
  const [skills, setSkills] = useState<SkillDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const refresh = useCallback(() => {
    if (!conn) return;
    conn.listSkills().then(
      (list) => {
        setSkills(list);
        setError(null);
      },
      (err) => {
        setSkills((current) => current ?? []);
        setError(`falha ao listar as skills: ${message(err)}`);
      },
    );
  }, [conn]);

  // Also re-runs after a reconnect, when `conn` is a new connection.
  useEffect(refresh, [refresh]);

  function handleSave() {
    if (!editor || !conn) return;
    setSaving(true);
    setError(null);
    // Accepting a suggestion is saving it without the mark; editing it without accepting keeps it a suggestion.
    const skill: SkillDto = editor.skill.proposed && editor.accept ? { ...editor.skill, proposed: false } : editor.skill;
    if (!skill.proposed) {
      delete skill.source;
      delete skill.proposedAt;
    }
    conn.saveSkill(skill, editor.mode === "edit").then(
      () => {
        setSaving(false);
        setEditor(null);
        refresh();
      },
      (err) => {
        setSaving(false);
        setError(`falha ao salvar a skill: ${message(err)}`);
      },
    );
  }

  /** Accepts a suggestion as it is. */
  function handleAccept(skill: SkillDto) {
    if (!conn) return;
    setError(null);
    const accepted: SkillDto = { name: skill.name, description: skill.description, body: skill.body, agents: skill.agents };
    conn.saveSkill(accepted, true).then(refresh, (err) => setError(`falha ao aceitar a skill: ${message(err)}`));
  }

  function handleDelete(name: string) {
    if (!conn) return;
    setError(null);
    conn.deleteSkill(name).then(
      () => {
        setConfirmDelete(null);
        refresh();
      },
      (err) => {
        setConfirmDelete(null);
        setError(`falha ao apagar a skill: ${message(err)}`);
      },
    );
  }

  function updateEditor(patch: Partial<SkillDto>) {
    setEditor((current) => (current ? { ...current, skill: { ...current.skill, ...patch } } : current));
  }

  const suggested = (skills ?? []).filter((s) => s.proposed);
  const active = (skills ?? []).filter((s) => !s.proposed);

  if (editor) {
    return (
      <form
        className="skills-editor"
        onSubmit={(e) => {
          e.preventDefault();
          handleSave();
        }}
      >
        <strong>{editor.mode === "new" ? "Nova skill" : `Editar ${editor.skill.name}`}</strong>
        <label>
          Nome
          <input
            value={editor.skill.name}
            onChange={(e) => updateEditor({ name: e.target.value })}
            placeholder="review-pr"
            disabled={editor.mode === "edit"}
            required
          />
          <span className="skills-hint">Minúsculas, números e hífens. Não muda depois de salvar.</span>
        </label>
        <label>
          Descrição
          <input
            value={editor.skill.description}
            onChange={(e) => updateEditor({ description: e.target.value })}
            placeholder="Uma frase: o que faz e quando usar"
            required
          />
        </label>
        <label>
          Instruções
          <textarea
            value={editor.skill.body}
            onChange={(e) => updateEditor({ body: e.target.value })}
            rows={10}
            placeholder="As instruções completas que a IA deve seguir, em markdown."
            required
          />
        </label>
        {editor.skill.agents.length > 0 && (
          <span className="skills-hint">Restrita aos agentes: {editor.skill.agents.join(", ")} (edite no desktop).</span>
        )}
        {editor.skill.proposed && (
          <label className="checkbox-row">
            <input type="checkbox" checked={editor.accept ?? false} onChange={(e) => setEditor({ ...editor, accept: e.target.checked })} />
            Aceitar esta skill: passa a valer nas conversas
          </label>
        )}
        {error && <p className="error-banner">{error}</p>}
        <div className="skills-actions">
          <button type="submit" className="primary-button" disabled={saving || !conn}>
            {saving ? "Salvando…" : "Salvar"}
          </button>
          <button
            type="button"
            className="link-button"
            disabled={saving}
            onClick={() => {
              setEditor(null);
              setError(null);
            }}
          >
            Cancelar
          </button>
        </div>
      </form>
    );
  }

  return (
    <div className="skills-view">
      <div className="skills-toolbar">
        <span className="skills-hint">Instruções que a IA carrega sozinha quando o pedido combina.</span>
        <button
          type="button"
          className="primary-button"
          onClick={() => {
            setError(null);
            setEditor({ mode: "new", skill: emptySkill });
          }}
        >
          + Nova
        </button>
      </div>
      {error && <p className="error-banner">{error}</p>}
      {user && user.learningEnabled && (
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={!user.learningOptOut}
            disabled={!conn}
            onChange={(e) => {
              const optOut = !e.target.checked;
              setError(null);
              conn?.setLearning(!optOut).then(
                () => onLearningChange?.(optOut),
                (err) => setError(`falha ao mudar o aprendizado: ${message(err)}`),
              );
            }}
          />
          A IA pode aprender com minhas conversas e sugerir skills (nada vale até eu aceitar)
        </label>
      )}
      {suggested.length > 0 && (
        <section>
          <h3>Sugeridas pela IA</h3>
          <p className="skills-hint">
            Depois de algumas conversas a IA sugeriu estas skills. <strong>Nada nelas vale até você aceitar</strong>: leia o texto, edite se precisar, e
            aceite ou rejeite.
          </p>
          <ul className="skills-list">
            {suggested.map((skill) => (
              <li key={skill.name} className="skills-item skills-item--suggested">
                <div className="skills-item-header">
                  <span className="skills-item-name">{skill.name}</span>
                  <span className="devices-status devices-status--pending">{skill.revises ? `Alteração de ${skill.revises}` : "Sugerida"}</span>
                </div>
                <p className="skills-item-description">{skill.description || "(sem descrição)"}</p>
                <p className="skills-hint">
                  {skill.source ? <>Da conversa <code>{skill.source}</code></> : "De uma conversa"}
                  {skill.proposedAt ? ` · ${new Date(skill.proposedAt).toLocaleString()}` : ""}
                  {skill.agents.length > 0 ? ` · só para: ${skill.agents.join(", ")}` : ""}
                </p>
                <details>
                  <summary>Ver as instruções</summary>
                  <pre className="skills-body">{skill.body}</pre>
                </details>
                {confirmDelete === skill.name ? (
                  <span className="skills-actions">
                    <button type="button" className="link-button skills-danger" onClick={() => handleDelete(skill.name)}>
                      Rejeitar mesmo
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(null)}>
                      Manter
                    </button>
                  </span>
                ) : (
                  <span className="skills-actions">
                    <button type="button" className="primary-button" disabled={!conn} onClick={() => handleAccept(skill)}>
                      Aceitar
                    </button>
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => {
                        setError(null);
                        setEditor({ mode: "edit", skill, accept: true });
                      }}
                    >
                      Editar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(skill.name)}>
                      Rejeitar
                    </button>
                  </span>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
      {skills === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : active.length === 0 ? (
        <p className="skills-hint">Nenhuma skill ainda.</p>
      ) : (
        <ul className="skills-list">
          {active.map((skill) => (
            <li key={skill.name} className="skills-item">
              <div className="skills-item-header">
                <span className="skills-item-name">{skill.name}</span>
                {confirmDelete === skill.name ? (
                  <span className="skills-actions">
                    <button type="button" className="link-button skills-danger" onClick={() => handleDelete(skill.name)}>
                      Apagar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(null)}>
                      Manter
                    </button>
                  </span>
                ) : (
                  <span className="skills-actions">
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => {
                        setError(null);
                        setEditor({ mode: "edit", skill });
                      }}
                    >
                      Editar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(skill.name)}>
                      Apagar
                    </button>
                  </span>
                )}
              </div>
              <p className="skills-item-description">{skill.description || "(sem descrição)"}</p>
              {skill.agents.length > 0 && <p className="skills-hint">Só para: {skill.agents.join(", ")}</p>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
