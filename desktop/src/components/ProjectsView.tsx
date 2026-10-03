import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ProjectEntry } from "../types";

const emptyProject: ProjectEntry = { id: "", name: "", description: "", instructions: "" };

interface EditorState {
  mode: "new" | "edit";
  project: ProjectEntry;
}

/** `Tax return 2026` → `tax-return-2026`: a first guess at the folder name, which the person can change until saving. */
function slugFrom(name: string): string {
  return name
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

/** The files of a project: the notes of the vault under `projects/<id>/`, listed here and added to from here
 * (anything else is done in the Vault screen, which sees the same files). */
function ProjectFiles({ projectId }: { projectId: string }) {
  const prefix = `projects/${projectId}/`;
  const [files, setFiles] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState<{ name: string; content: string } | null>(null);
  const [busy, setBusy] = useState(false);

  function refresh() {
    invoke<string[]>("list_vault_files")
      .then((all) => {
        setFiles(all.filter((path) => path.startsWith(prefix) && path !== `${prefix}PROJECT.md`).map((path) => path.slice(prefix.length)));
        setError(null);
      })
      .catch((err) => setError(String(err)));
  }

  useEffect(refresh, [projectId]);

  async function saveDraft() {
    if (!draft) return;
    setBusy(true);
    setError(null);
    try {
      const name = draft.name.trim().replace(/^\/+/, "");
      // No `expectedVersion`: creating, so an existing note of that name is refused instead of overwritten.
      await invoke("save_vault_note", { path: `${prefix}${name}`, content: draft.content, expectedVersion: null });
      setDraft(null);
      refresh();
    } catch (err) {
      setError(typeof err === "object" && err !== null && "message" in err ? String((err as { message: unknown }).message) : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="settings-field">
      <span className="settings-label">Files</span>
      <span className="settings-hint">
        Notes in <code>{prefix}</code> in your vault. In this project's conversations the AI reads and writes only these files.
      </span>
      {error && <p className="settings-error-banner">{error}</p>}
      {files === null ? (
        <span className="settings-hint">Loading…</span>
      ) : files.length === 0 ? (
        <span className="settings-hint">No files yet.</span>
      ) : (
        <ul className="project-file-list">
          {files.map((file) => (
            <li key={file}>
              <code>{file}</code>
            </li>
          ))}
        </ul>
      )}
      {draft ? (
        <div className="skill-file-draft">
          <input
            className="settings-input"
            type="text"
            placeholder="notes.md"
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.currentTarget.value })}
          />
          <textarea
            className="settings-input settings-textarea"
            rows={8}
            placeholder="The note's text."
            value={draft.content}
            onChange={(e) => setDraft({ ...draft, content: e.currentTarget.value })}
          />
          <div className="skill-editor-actions">
            <button type="button" className="settings-save-btn" onClick={saveDraft} disabled={busy || draft.name.trim() === ""}>
              {busy ? "Saving…" : "Save note"}
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setDraft(null)} disabled={busy}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <div>
          <button type="button" className="settings-browse-btn" onClick={() => setDraft({ name: "", content: "" })}>
            + Add a note
          </button>
        </div>
      )}
    </div>
  );
}

function ProjectsView({ onChanged }: { onChanged: () => void }) {
  const [projects, setProjects] = useState<ProjectEntry[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  // Whether the id was typed by hand: until then it follows the name.
  const [idTouched, setIdTouched] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  function refresh() {
    invoke<ProjectEntry[]>("list_projects")
      .then((list) => {
        setProjects(list);
        setLoadError(null);
      })
      .catch((err) => setLoadError(String(err)));
    onChanged();
  }

  useEffect(refresh, []);

  function openNew() {
    setError(null);
    setIdTouched(false);
    setEditor({ mode: "new", project: emptyProject });
  }

  function openEdit(project: ProjectEntry) {
    setError(null);
    setEditor({ mode: "edit", project });
  }

  function updateEditor(patch: Partial<ProjectEntry>) {
    setEditor((current) => (current ? { ...current, project: { ...current.project, ...patch } } : current));
  }

  async function handleSave() {
    if (!editor) return;
    setError(null);
    setSaving(true);
    try {
      await invoke("save_project", { project: editor.project, overwrite: editor.mode === "edit" });
      setEditor(null);
      refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete(id: string) {
    setError(null);
    try {
      await invoke("delete_project", { id });
      setConfirmDelete(null);
      refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="settings-view">
      <h2 className="settings-title">Projects</h2>
      <p className="settings-hint">
        A project groups conversations around one subject, with instructions and files of its own. In a project's
        conversations the AI is told the instructions and works only on the project's files — it doesn't see the rest of
        your vault, your skills, or other conversations, and it gets no shell. Pick the project when you start a
        conversation; it can't be changed afterwards. Projects are plain folders in <code>projects/</code> in your vault, so
        they sync with it.
      </p>

      {loadError && <p className="settings-error-banner">{loadError}</p>}
      {error && <p className="settings-error-banner">{error}</p>}

      {editor && (
        <div className="provider-card skill-editor">
          <span className="settings-section-title">{editor.mode === "new" ? "New project" : `Edit ${editor.project.name}`}</span>

          <label className="settings-field">
            <span className="settings-label">Name</span>
            <input
              className="settings-input"
              type="text"
              placeholder="Tax return 2026"
              value={editor.project.name}
              onChange={(e) => {
                const name = e.currentTarget.value;
                updateEditor(editor.mode === "new" && !idTouched ? { name, id: slugFrom(name) } : { name });
              }}
            />
          </label>

          <label className="settings-field">
            <span className="settings-label">Folder name</span>
            <input
              className="settings-input"
              type="text"
              placeholder="tax-return-2026"
              value={editor.project.id}
              disabled={editor.mode === "edit"}
              onChange={(e) => {
                setIdTouched(true);
                updateEditor({ id: e.currentTarget.value });
              }}
            />
            <span className="settings-hint">Letters, digits, '-' and '_'. It can't be changed after saving.</span>
          </label>

          <label className="settings-field">
            <span className="settings-label">Description</span>
            <input
              className="settings-input"
              type="text"
              placeholder="One sentence: what this project is about"
              value={editor.project.description}
              onChange={(e) => updateEditor({ description: e.currentTarget.value })}
            />
          </label>

          <label className="settings-field">
            <span className="settings-label">Instructions</span>
            <textarea
              className="settings-input settings-textarea"
              rows={10}
              placeholder="What the AI should know and follow in every conversation of this project, in markdown."
              value={editor.project.instructions}
              onChange={(e) => updateEditor({ instructions: e.currentTarget.value })}
            />
            <span className="settings-hint">Sent with every message of the project's conversations, so keep it short (up to 16 KB).</span>
          </label>

          {editor.mode === "edit" ? (
            <ProjectFiles projectId={editor.project.id} />
          ) : (
            <span className="settings-hint">Save the project first, then you can add files to it.</span>
          )}

          <div className="skill-editor-actions">
            <button type="button" className="settings-save-btn" onClick={handleSave} disabled={saving}>
              {saving ? "Saving…" : "Save project"}
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setEditor(null)} disabled={saving}>
              Cancel
            </button>
          </div>
        </div>
      )}

      <section className="settings-section">
        <div className="settings-section-header">
          <h3 className="settings-section-title">Your projects</h3>
          <button type="button" className="settings-browse-btn" onClick={openNew}>
            + New project
          </button>
        </div>

        {projects === null ? (
          <p className="settings-hint">Loading projects…</p>
        ) : projects.length === 0 ? (
          <p className="settings-hint">No projects yet.</p>
        ) : (
          <div className="provider-list">
            {projects.map((project) => (
              <div className="provider-card skill-card" key={project.id}>
                <div className="skill-card-header">
                  <span className="skill-card-name">{project.name}</span>
                  <span className="settings-hint">
                    <code>{project.id}</code>
                  </span>
                  <div className="skill-card-actions">
                    {confirmDelete === project.id ? (
                      <>
                        <span className="settings-hint">Remove this project? Its files stay in the vault.</span>
                        <button type="button" className="provider-delete-btn" onClick={() => handleDelete(project.id)}>
                          Remove
                        </button>
                        <button type="button" className="settings-browse-btn" onClick={() => setConfirmDelete(null)}>
                          Keep
                        </button>
                      </>
                    ) : (
                      <>
                        <button type="button" className="settings-browse-btn" onClick={() => openEdit(project)}>
                          Edit
                        </button>
                        <button
                          type="button"
                          className="provider-delete-btn"
                          onClick={() => setConfirmDelete(project.id)}
                          aria-label={`Remove ${project.name}`}
                          title="Remove this project"
                        >
                          🗑
                        </button>
                      </>
                    )}
                  </div>
                </div>
                <p className="skill-card-description">{project.description || "(no description)"}</p>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

export default ProjectsView;
