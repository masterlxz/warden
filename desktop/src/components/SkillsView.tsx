import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentEntry, ProviderEntry, SkillEntry } from "../types";

/** What the editor form is doing: `new` (name editable, refuses a taken name) or `edit` (name
 * locked, overwrites). `fromAi` only drives the "review before saving" hint. */
interface EditorState {
  mode: "new" | "edit";
  skill: SkillEntry;
  fromAi: boolean;
}

const emptySkill: SkillEntry = { name: "", description: "", body: "", agents: [] };

/** An attachment being written: `existing` locks the name (it's an edit of a file already there). */
interface FileDraft {
  name: string;
  content: string;
  existing: boolean;
}

/** Text files attached to a saved skill (P72 d) — scripts, templates, notes the AI can read through
 * `read_skill_file`. Changes hit the vault straight away, independent of the form's "Save skill". */
function SkillFiles({ skillName }: { skillName: string }) {
  const [files, setFiles] = useState<string[] | null>(null);
  const [draft, setDraft] = useState<FileDraft | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  function refresh() {
    invoke<string[]>("list_skill_files", { name: skillName })
      .then(setFiles)
      .catch((err) => setError(String(err)));
  }

  useEffect(refresh, [skillName]);

  async function openFile(file: string) {
    setError(null);
    try {
      const content = await invoke<string>("read_skill_attachment", { name: skillName, file });
      setDraft({ name: file, content, existing: true });
    } catch (err) {
      setError(String(err));
    }
  }

  async function saveDraft() {
    if (!draft) return;
    setError(null);
    setBusy(true);
    try {
      await invoke("save_skill_attachment", { name: skillName, file: draft.name, content: draft.content });
      setDraft(null);
      refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function removeFile(file: string) {
    setError(null);
    try {
      await invoke("delete_skill_attachment", { name: skillName, file });
      setConfirmDelete(null);
      refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="settings-field">
      <span className="settings-label">Attached files</span>
      <span className="settings-hint">
        Text files the AI can read when it uses this skill, such as a script or a template (up to 20 files, 64 KB
        each). A script only runs if the shell tool is enabled in Settings.
      </span>
      {error && <p className="settings-error-banner">{error}</p>}
      {files === null ? null : files.length === 0 ? (
        <span className="settings-hint">No files attached.</span>
      ) : (
        <div className="skill-file-list">
          {files.map((file) => (
            <div className="skill-file-row" key={file}>
              <span className="skill-card-name">{file}</span>
              <div className="skill-card-actions">
                {confirmDelete === file ? (
                  <>
                    <span className="settings-hint">Remove?</span>
                    <button type="button" className="provider-delete-btn" onClick={() => removeFile(file)}>
                      Remove
                    </button>
                    <button type="button" className="settings-browse-btn" onClick={() => setConfirmDelete(null)}>
                      Keep
                    </button>
                  </>
                ) : (
                  <>
                    <button type="button" className="settings-browse-btn" onClick={() => openFile(file)}>
                      Edit
                    </button>
                    <button
                      type="button"
                      className="provider-delete-btn"
                      onClick={() => setConfirmDelete(file)}
                      aria-label={`Remove ${file}`}
                      title="Remove this file"
                    >
                      🗑
                    </button>
                  </>
                )}
              </div>
            </div>
          ))}
        </div>
      )}

      {draft ? (
        <div className="skill-file-draft">
          <input
            className="settings-input"
            type="text"
            placeholder="run.sh"
            value={draft.name}
            disabled={draft.existing}
            onChange={(e) => setDraft({ ...draft, name: e.currentTarget.value })}
          />
          <textarea
            className="settings-input settings-textarea"
            rows={8}
            placeholder="File content (text)."
            value={draft.content}
            onChange={(e) => setDraft({ ...draft, content: e.currentTarget.value })}
          />
          <div className="skill-editor-actions">
            <button
              type="button"
              className="settings-save-btn"
              onClick={saveDraft}
              disabled={busy || draft.name.trim() === ""}
            >
              {busy ? "Saving…" : "Save file"}
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setDraft(null)} disabled={busy}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <div>
          <button
            type="button"
            className="settings-browse-btn"
            onClick={() => setDraft({ name: "", content: "", existing: false })}
          >
            + Attach a file
          </button>
        </div>
      )}
    </div>
  );
}

function SkillsView({
  agents,
  providers,
  activeProvider,
}: {
  agents: AgentEntry[];
  providers: ProviderEntry[];
  activeProvider: string;
}) {
  const [skills, setSkills] = useState<SkillEntry[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [prompt, setPrompt] = useState("");
  const [generating, setGenerating] = useState(false);
  // Which provider drafts the skill; "" = the active one (same convention as the chat's model picker).
  const [draftProvider, setDraftProvider] = useState("");
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
      const providerId = draftProvider === "" || draftProvider === activeProvider ? null : draftProvider;
      const draft = await invoke<SkillEntry>("generate_skill_draft", { prompt, providerId });
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

  /** An id already on the skill but no longer in the registry (agent deleted/renamed) stays
   * listed, so saving doesn't silently drop it — the user can untick it deliberately. */
  function toggleAgent(id: string, checked: boolean) {
    if (!editor) return;
    const current = editor.skill.agents;
    updateEditor({ agents: checked ? [...current.filter((a) => a !== id), id] : current.filter((a) => a !== id) });
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
        {providers.length > 1 && (
          <label className="settings-field">
            <span className="settings-label">Model</span>
            <select
              className="settings-select"
              value={draftProvider}
              onChange={(e) => setDraftProvider(e.currentTarget.value)}
            >
              <option value="">Active provider{activeProvider ? ` (${activeProvider})` : ""}</option>
              {providers
                .filter((p) => p.id !== activeProvider)
                .map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.id}
                  </option>
                ))}
            </select>
          </label>
        )}
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

          <div className="settings-field">
            <span className="settings-label">Available to</span>
            {(() => {
              const known = new Set(agents.map((a) => a.id));
              const ids = [...agents.map((a) => a.id), ...editor.skill.agents.filter((id) => !known.has(id))];
              return ids.length === 0 ? (
                <span className="settings-hint">No agents configured — every skill is visible to every conversation.</span>
              ) : (
                <div className="skill-agent-list">
                  {ids.map((id) => (
                    <label className="skill-agent-option" key={id}>
                      <input
                        type="checkbox"
                        checked={editor.skill.agents.includes(id)}
                        onChange={(e) => toggleAgent(id, e.currentTarget.checked)}
                      />
                      {id}
                      {!known.has(id) && <span className="settings-hint"> (agent no longer exists)</span>}
                    </label>
                  ))}
                </div>
              );
            })()}
            <span className="settings-hint">
              None ticked means every agent sees it. Ticked means only those agents — chats without an agent won't
              see it.
            </span>
          </div>

          {editor.mode === "edit" ? (
            <SkillFiles skillName={editor.skill.name} />
          ) : (
            <span className="settings-hint">Save the skill first, then you can attach files to it.</span>
          )}

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
                {skill.agents.length > 0 && <p className="settings-hint">Only for: {skill.agents.join(", ")}</p>}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

export default SkillsView;
