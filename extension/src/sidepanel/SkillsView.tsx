import { useEffect, useState } from "react";
import type { SkillDto } from "../protocol/messages";
import type { ListSkillsResponse, OkResponse } from "../background/popup_protocol";

/** What the editor form is doing: `new` (name editable, refuses a taken name) or `edit` (name
 * locked, overwrites) — same split as the desktop's SkillsView. */
interface EditorState {
  mode: "new" | "edit";
  skill: SkillDto;
  /** Editing a suggestion: saving accepts it only when this is on. */
  accept?: boolean;
}

const emptySkill: SkillDto = { name: "", description: "", body: "", agents: [] };

export default function SkillsView() {
  const [skills, setSkills] = useState<SkillDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  function refresh() {
    chrome.runtime.sendMessage({ type: "listSkills" }).then((res: ListSkillsResponse) => {
      if (res.ok) {
        setSkills(res.skills);
        setError(null);
      } else {
        setSkills((current) => current ?? []);
        setError(res.error ?? "falha ao listar as skills");
      }
    });
  }

  useEffect(refresh, []);

  function handleSave() {
    if (!editor) return;
    setSaving(true);
    setError(null);
    // Editing a suggestion without accepting keeps it pending; the hub reads a missing `proposed` as accepted.
    const { proposed, source, proposedAt, ...accepted } = editor.skill;
    const skill = proposed && !editor.accept ? editor.skill : accepted;
    void source;
    void proposedAt;
    chrome.runtime
      .sendMessage({ type: "saveSkill", skill, overwrite: editor.mode === "edit" })
      .then((res: OkResponse) => {
        setSaving(false);
        if (res.ok) {
          setEditor(null);
          refresh();
        } else {
          setError(res.error ?? "falha ao salvar a skill");
        }
      });
  }

  /** Accepts a suggestion as it is: a save without the `proposed` mark. */
  function handleAccept(skill: SkillDto) {
    setError(null);
    const accepted: SkillDto = { name: skill.name, description: skill.description, body: skill.body, agents: skill.agents };
    chrome.runtime.sendMessage({ type: "saveSkill", skill: accepted, overwrite: true }).then((res: OkResponse) => {
      if (res.ok) refresh();
      else setError(res.error ?? "falha ao aceitar a skill");
    });
  }

  function handleDelete(name: string) {
    setError(null);
    chrome.runtime.sendMessage({ type: "deleteSkill", name }).then((res: OkResponse) => {
      setConfirmDelete(null);
      if (res.ok) refresh();
      else setError(res.error ?? "falha ao apagar a skill");
    });
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
        {editor.skill.proposed && (
          <label>
            <input type="checkbox" checked={editor.accept ?? false} onChange={(e) => setEditor({ ...editor, accept: e.target.checked })} />
            Aceitar esta skill: passa a valer nas conversas
          </label>
        )}
        {error && <p className="error-banner">{error}</p>}
        <div className="skills-actions">
          <button type="submit" disabled={saving}>
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
                {skill.proposed && <span className="skills-hint">{skill.revises ? `alteração sugerida para ${skill.revises} — não vale até aceitar` : "sugerida pela IA — não vale até aceitar"}</span>}
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
                    {skill.proposed && (
                      <button type="button" className="link-button" onClick={() => handleAccept(skill)}>
                        Aceitar
                      </button>
                    )}
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => {
                        setError(null);
                        setEditor({ mode: "edit", skill, accept: skill.proposed ? true : undefined });
                      }}
                    >
                      Editar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(skill.name)}>
                      {skill.proposed ? "Rejeitar" : "Apagar"}
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
