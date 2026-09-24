import { useCallback, useEffect, useState } from "react";
import type { ServerConnection } from "../hub/connection";
import type { SkillDto } from "../hub/messages";

// Adapted from the extension's `sidepanel/SkillsView.tsx` (P78), talking to the hub directly
// instead of through a background service worker.

/** What the editor form is doing: `new` (name editable, refuses a taken name) or `edit` (name
 * locked, overwrites) — same split as the desktop's SkillsView. */
interface EditorState {
  mode: "new" | "edit";
  skill: SkillDto;
}

const emptySkill: SkillDto = { name: "", description: "", body: "", agents: [] };

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SkillsView({ conn }: { conn: ServerConnection | null }) {
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
    conn.saveSkill(editor.skill, editor.mode === "edit").then(
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
      {skills === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : skills.length === 0 ? (
        <p className="skills-hint">Nenhuma skill ainda.</p>
      ) : (
        <ul className="skills-list">
          {skills.map((skill) => (
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
