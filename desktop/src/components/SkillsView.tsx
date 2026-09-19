import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SkillEntry } from "../types";

/** What the editor form is doing: `new` (name editable, refuses a taken name) or `edit` (name
 * locked, overwrites). `fromAi` only drives the "review before saving" hint. */
interface EditorState {
  mode: "new" | "edit";
  skill: SkillEntry;
  fromAi: boolean;
}

const emptySkill: SkillEntry = { name: "", description: "", body: "" };

function SkillsView() {
  const [skills, setSkills] = useState<SkillEntry[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [prompt, setPrompt] = useState("");
  const [generating, setGenerating] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  function refresh() {
    invoke<SkillEntry[]>("list_skills")
      .then((list) => {
        setSkills(list);
        setLoadError(null);
      })
      .catch((err) => setLoadError(String(err)));
  }

  useEffect(refresh, []);

  function openNew() {
    setError(null);
    setEditor({ mode: "new", skill: emptySkill, fromAi: false });
  }

  function openEdit(skill: SkillEntry) {
    setError(null);
    setEditor({ mode: "edit", skill, fromAi: false });
  }

  async function handleGenerate() {
    setError(null);
    setGenerating(true);
    try {
      const draft = await invoke<SkillEntry>("generate_skill_draft", { prompt, providerId: null });
      setEditor({ mode: "new", skill: draft, fromAi: true });
    } catch (err) {
      setError(String(err));
    } finally {
      setGenerating(false);
    }
  }

  async function handleSave() {
    if (!editor) return;
    setError(null);
    setSaving(true);
    try {
      await invoke("save_skill", { skill: editor.skill, overwrite: editor.mode === "edit" });
      setEditor(null);
      setPrompt("");
      refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete(name: string) {
    setError(null);
    try {
      await invoke("delete_skill", { name });
      setConfirmDelete(null);
      refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  function updateEditor(patch: Partial<SkillEntry>) {
    setEditor((current) => (current ? { ...current, skill: { ...current.skill, ...patch } } : current));
  }

  return (
    <div className="settings-view">
      <h2 className="settings-title">Skills</h2>
      <p className="settings-hint">
        Skills are reusable instructions the AI loads on its own when a request matches one — they cost nothing in a
        conversation until used. Create one here, describe one and let the AI draft it, or just ask for one in a chat.
        They're stored as plain markdown in <code>skills/</code> inside your vault, so they sync with it.
      </p>

      {loadError && <p className="settings-error-banner">{loadError}</p>}
      {error && <p className="settings-error-banner">{error}</p>}

      <section className="settings-section">
        <div className="settings-section-header">
          <h3 className="settings-section-title">Describe a skill</h3>
        </div>
        <label className="settings-field">
          <span className="settings-hint">
            Say roughly what it should do — the AI writes a draft you can review and edit before saving.
          </span>
          <textarea
            className="settings-input settings-textarea"
            rows={3}
            placeholder="e.g. 'How to review a pull request: check tests first, then naming, then security, and answer with a short verdict plus a bullet list of issues.'"
            value={prompt}
            onChange={(e) => setPrompt(e.currentTarget.value)}
          />
        </label>
        <button
          type="button"
          className="settings-save-btn"
          onClick={handleGenerate}
          disabled={generating || prompt.trim() === ""}
        >
          {generating ? "Drafting…" : "Generate draft"}
        </button>
      </section>

      {editor && (
        <div className="provider-card skill-editor">
          <span className="settings-section-title">{editor.mode === "new" ? "New skill" : `Edit ${editor.skill.name}`}</span>
          {editor.fromAi && (
            <p className="settings-hint">AI draft — read it through and adjust anything before saving.</p>
          )}

          <label className="settings-field">
            <span className="settings-label">Name</span>
            <input
              className="settings-input"
              type="text"
              placeholder="review-pr"
              value={editor.skill.name}
              disabled={editor.mode === "edit"}
              onChange={(e) => updateEditor({ name: e.currentTarget.value })}
            />
            <span className="settings-hint">Lowercase letters, digits and hyphens. It can't be changed after saving.</span>
          </label>

          <label className="settings-field">
            <span className="settings-label">Description</span>
            <input
              className="settings-input"
              type="text"
              placeholder="One sentence: what it does and when to use it"
              value={editor.skill.description}
              onChange={(e) => updateEditor({ description: e.currentTarget.value })}
            />
          </label>

          <label className="settings-field">
            <span className="settings-label">Instructions</span>
            <textarea
              className="settings-input settings-textarea"
              rows={10}
              placeholder="The full instructions the AI should follow, in markdown."
              value={editor.skill.body}
              onChange={(e) => updateEditor({ body: e.currentTarget.value })}
            />
          </label>

          <div className="skill-editor-actions">
            <button type="button" className="settings-save-btn" onClick={handleSave} disabled={saving}>
              {saving ? "Saving…" : "Save skill"}
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setEditor(null)} disabled={saving}>
              Cancel
            </button>
          </div>
        </div>
      )}

      <section className="settings-section">
        <div className="settings-section-header">
          <h3 className="settings-section-title">Your skills</h3>
          <button type="button" className="settings-browse-btn" onClick={openNew}>
            + New skill
          </button>
        </div>

        {skills === null ? (
          <p className="settings-hint">Loading skills…</p>
        ) : skills.length === 0 ? (
          <p className="settings-hint">No skills yet.</p>
        ) : (
          <div className="provider-list">
            {skills.map((skill) => (
              <div className="provider-card skill-card" key={skill.name}>
                <div className="skill-card-header">
                  <span className="skill-card-name">{skill.name}</span>
                  <div className="skill-card-actions">
                    {confirmDelete === skill.name ? (
                      <>
                        <span className="settings-hint">Delete this skill?</span>
                        <button type="button" className="provider-delete-btn" onClick={() => handleDelete(skill.name)}>
                          Delete
                        </button>
                        <button type="button" className="settings-browse-btn" onClick={() => setConfirmDelete(null)}>
                          Keep
                        </button>
                      </>
                    ) : (
                      <>
                        <button type="button" className="settings-browse-btn" onClick={() => openEdit(skill)}>
                          Edit
                        </button>
                        <button
                          type="button"
                          className="provider-delete-btn"
                          onClick={() => setConfirmDelete(skill.name)}
                          aria-label={`Delete ${skill.name}`}
                          title="Delete this skill"
                        >
                          🗑
                        </button>
                      </>
                    )}
                  </div>
                </div>
                <p className="skill-card-description">{skill.description || "(no description)"}</p>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

export default SkillsView;
