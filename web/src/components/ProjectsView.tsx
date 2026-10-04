import { useCallback, useEffect, useState } from "react";
import type { ServerConnection } from "../hub/connection";
import type { ProjectDto } from "../hub/messages";

/** What the editor form is doing: `new` (id editable, refuses a taken one) or `edit` (id locked, overwrites) — the same
 * split as the Skills screen. */
interface EditorState {
  mode: "new" | "edit";
  project: ProjectDto;
}

const emptyProject: ProjectDto = { id: "", name: "", description: "", instructions: "" };

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** `Declaração 2026` → `declaracao-2026`: a first guess at the folder name, which can be changed until saving. */
function slugFrom(name: string): string {
  return name
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

/** The files of a project: the vault's notes under `projects/<id>/`, listed here and added to from here (the Vault tab
 * sees the same files). */
function ProjectFiles({ conn, projectId }: { conn: ServerConnection | null; projectId: string }) {
  const prefix = `projects/${projectId}/`;
  const [files, setFiles] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState<{ name: string; content: string } | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    if (!conn) return;
    conn.listVaultFiles().then(
      (all) => {
        setFiles(all.filter((path) => path.startsWith(prefix) && path !== `${prefix}PROJECT.md`).map((path) => path.slice(prefix.length)));
        setError(null);
      },
      (err) => setError(`falha ao listar os arquivos: ${message(err)}`),
    );
  }, [conn, prefix]);

  useEffect(refresh, [refresh]);

  function saveDraft() {
    if (!draft || !conn) return;
    setBusy(true);
    setError(null);
    // No expected version: this creates the note, so one that exists is refused instead of overwritten.
    conn.saveVaultNote(`${prefix}${draft.name.trim().replace(/^\/+/, "")}`, draft.content).then(
      () => {
        setBusy(false);
        setDraft(null);
        refresh();
      },
      (err) => {
        setBusy(false);
        setError(`falha ao salvar a nota: ${message(err)}`);
      },
    );
  }

  return (
    <div className="project-files">
      <strong>Arquivos</strong>
      <span className="skills-hint">
        Notas em <code>{prefix}</code> no cofre. Nas conversas do projeto a IA lê e escreve só nestes arquivos.
      </span>
      {error && <p className="error-banner">{error}</p>}
      {files === null ? (
        <span className="skills-hint">Carregando…</span>
      ) : files.length === 0 ? (
        <span className="skills-hint">Nenhum arquivo ainda.</span>
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
        <div className="project-file-draft">
          <input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} placeholder="notas.md" aria-label="Nome do arquivo" />
          <textarea value={draft.content} onChange={(e) => setDraft({ ...draft, content: e.target.value })} rows={8} placeholder="O texto da nota." aria-label="Texto da nota" />
          <div className="skills-actions">
            <button type="button" className="primary-button" onClick={saveDraft} disabled={busy || !conn || draft.name.trim() === ""}>
              {busy ? "Salvando…" : "Salvar nota"}
            </button>
            <button type="button" className="link-button" onClick={() => setDraft(null)} disabled={busy}>
              Cancelar
            </button>
          </div>
        </div>
      ) : (
        <div>
          <button type="button" className="link-button" onClick={() => setDraft({ name: "", content: "" })}>
            + Adicionar nota
          </button>
        </div>
      )}
    </div>
  );
}

/** `onChanged` gets the list just read, so the chat's picker and list follow what was made, edited or removed here. It
 * must be a stable function (a state setter): `refresh` depends on it. */
export default function ProjectsView({ conn, onChanged }: { conn: ServerConnection | null; onChanged: (projects: ProjectDto[]) => void }) {
  const [projects, setProjects] = useState<ProjectDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  // Whether the id was typed by hand: until then it follows the name.
  const [idTouched, setIdTouched] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const refresh = useCallback(() => {
    if (!conn) return;
    conn.listProjects().then(
      (list) => {
        setProjects(list);
        setError(null);
        onChanged(list);
      },
      (err) => {
        setProjects((current) => current ?? []);
        setError(`falha ao listar os projetos: ${message(err)}`);
      },
    );
  }, [conn, onChanged]);

  // Also re-runs after a reconnect, when `conn` is a new connection.
  useEffect(refresh, [refresh]);

  function updateEditor(patch: Partial<ProjectDto>) {
    setEditor((current) => (current ? { ...current, project: { ...current.project, ...patch } } : current));
  }

  function handleSave() {
    if (!editor || !conn) return;
    setSaving(true);
    setError(null);
    conn.saveProject(editor.project, editor.mode === "edit").then(
      () => {
        setSaving(false);
        setEditor(null);
        refresh();
      },
      (err) => {
        setSaving(false);
        setError(`falha ao salvar o projeto: ${message(err)}`);
      },
    );
  }

  function handleDelete(id: string) {
    if (!conn) return;
    setError(null);
    conn.deleteProject(id).then(
      () => {
        setConfirmDelete(null);
        refresh();
      },
      (err) => {
        setConfirmDelete(null);
        setError(`falha ao remover o projeto: ${message(err)}`);
      },
    );
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
        <strong>{editor.mode === "new" ? "Novo projeto" : `Editar ${editor.project.name}`}</strong>
        <label>
          Nome
          <input
            value={editor.project.name}
            onChange={(e) => {
              const name = e.target.value;
              updateEditor(editor.mode === "new" && !idTouched ? { name, id: slugFrom(name) } : { name });
            }}
            placeholder="Declaração 2026"
            required
          />
        </label>
        <label>
          Nome da pasta
          <input
            value={editor.project.id}
            onChange={(e) => {
              setIdTouched(true);
              updateEditor({ id: e.target.value });
            }}
            placeholder="declaracao-2026"
            disabled={editor.mode === "edit"}
            required
          />
          <span className="skills-hint">Letras, números, «-» e «_». Não muda depois de salvar.</span>
        </label>
        <label>
          Descrição
          <input value={editor.project.description} onChange={(e) => updateEditor({ description: e.target.value })} placeholder="Uma frase: do que trata o projeto" />
        </label>
        <label>
          Pasta de trabalho (opcional)
          <input
            value={editor.project.workdir ?? ""}
            onChange={(e) => updateEditor({ workdir: e.target.value })}
            placeholder="/home/voce/codigo/meu-repo"
          />
          <span className="skills-hint">Caminho absoluto na máquina do hub. Com ela, as conversas do projeto ganham um terminal que começa ali e pede a sua aprovação a cada comando.</span>
        </label>
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={editor.project.code ?? false}
            disabled={!(editor.project.workdir ?? "").trim()}
            onChange={(e) => updateEditor({ code: e.target.checked })}
          />
          Modo código (opencode)
        </label>
        <span className="skills-hint">Precisa da pasta de trabalho e do opencode instalado na máquina do hub. As conversas do projeto passam a ser sessões do opencode nessa pasta, e cada ação dele pede a sua aprovação.</span>
        <label>
          Instruções
          <textarea
            value={editor.project.instructions}
            onChange={(e) => updateEditor({ instructions: e.target.value })}
            rows={10}
            placeholder="O que a IA deve saber e seguir em toda conversa deste projeto, em markdown."
          />
          <span className="skills-hint">Vão a cada mensagem das conversas do projeto, então seja breve (até 16 KB).</span>
        </label>
        {editor.mode === "edit" ? (
          <ProjectFiles conn={conn} projectId={editor.project.id} />
        ) : (
          <span className="skills-hint">Salve o projeto primeiro; depois dá para adicionar arquivos.</span>
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
        <span className="skills-hint">
          Um projeto agrupa conversas de um assunto, com instruções e arquivos próprios. Nas conversas dele a IA recebe as instruções e trabalha só nos arquivos
          do projeto: não vê o resto do cofre, as skills nem outras conversas, e não ganha terminal. O projeto se escolhe ao começar a conversa e não muda depois.
        </span>
        <button
          type="button"
          className="primary-button"
          onClick={() => {
            setError(null);
            setIdTouched(false);
            setEditor({ mode: "new", project: emptyProject });
          }}
        >
          + Novo
        </button>
      </div>
      {error && <p className="error-banner">{error}</p>}
      {projects === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : projects.length === 0 ? (
        <p className="skills-hint">Nenhum projeto ainda.</p>
      ) : (
        <ul className="skills-list">
          {projects.map((project) => (
            <li key={project.id} className="skills-item">
              <div className="skills-item-header">
                <span className="skills-item-name">{project.name}</span>
                <code className="skills-hint">{project.id}</code>
                {confirmDelete === project.id ? (
                  <span className="skills-actions">
                    <span className="skills-hint">Os arquivos ficam no cofre.</span>
                    <button type="button" className="link-button skills-danger" onClick={() => handleDelete(project.id)}>
                      Remover mesmo
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
                        setEditor({ mode: "edit", project });
                      }}
                    >
                      Editar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(project.id)}>
                      Remover
                    </button>
                  </span>
                )}
              </div>
              <p className="skills-item-description">{project.description || "(sem descrição)"}</p>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
