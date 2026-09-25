import { useEffect, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { MarkdownLink } from "./MessageBubble";

/** Same 3 reserved filenames/order as `warden_core::memory::FIXED_VAULT_FILES` and the headings
 * `Vault::standing_memory` builds from them — hardcoded here rather than fetched, since this is a
 * stable, documented contract (P52 part 1) unlikely to change. Labels are in Portuguese, matching
 * the actual template each file is seeded with (its own first line is a Portuguese `# ` title) —
 * screen chrome around them stays English, same split already used elsewhere (agent personas,
 * vault content, are whatever language the user wrote them in; the app's own UI text is English). */
const FIXED_FILES: { file: string; label: string }[] = [
  { file: "_profile.md", label: "Perfil do usuário" },
  { file: "_behavior.md", label: "Comportamento da IA" },
  { file: "_feedback.md", label: "Feedback e lições aprendidas" },
];

interface TreeNode {
  name: string;
  path: string;
  isFile: boolean;
  children: TreeNode[];
}

/** A note as opened — `version` goes back with a save or delete (P78), so a change made meanwhile
 * (by the AI, sync, or the web UI) is refused instead of overwritten. */
interface VaultNote {
  content: string;
  version: string;
}

interface VaultSearchHit {
  path: string;
  lineNumber: number;
  line: string;
}

/** What `vault_cmds.rs` rejects with. `conflict`: the note changed since it was opened. */
interface VaultCmdError {
  message: string;
  conflict: boolean;
}

function errorOf(err: unknown): VaultCmdError {
  if (typeof err === "object" && err !== null && "message" in err) {
    const e = err as { message: unknown; conflict?: unknown };
    return { message: String(e.message), conflict: e.conflict === true };
  }
  return { message: String(err), conflict: false };
}

/** Groups a flat list of relative paths (as `list_vault_files` returns them) into a folder tree —
 * folders before files, alphabetical (case-insensitive) within each level. Pure and DOM-free so
 * it's testable on its own. */
export function buildTree(paths: string[]): TreeNode[] {
  const roots: TreeNode[] = [];

  for (const path of paths) {
    const parts = path.split("/");
    let level = roots;
    let acc = "";
    parts.forEach((part, i) => {
      acc = acc ? `${acc}/${part}` : part;
      const isFile = i === parts.length - 1;
      let node = level.find((n) => n.name === part && n.isFile === isFile);
      if (!node) {
        node = { name: part, path: acc, isFile, children: [] };
        level.push(node);
      }
      level = node.children;
    });
  }

  sortTree(roots);
  return roots;
}

function sortTree(nodes: TreeNode[]) {
  nodes.sort((a, b) => {
    if (a.isFile !== b.isFile) return a.isFile ? 1 : -1;
    return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  });
  nodes.forEach((n) => sortTree(n.children));
}

/** A new note's path: trimmed, no leading slashes, `.md` added when it has no extension. */
function normalizeNewPath(input: string): string {
  const path = input.trim().replace(/^\/+/, "");
  const name = path.split("/").pop() ?? "";
  return name.includes(".") ? path : `${path}.md`;
}

function VaultTree({
  nodes,
  depth,
  selectedPath,
  onSelect,
}: {
  nodes: TreeNode[];
  depth: number;
  selectedPath: string | null;
  onSelect: (path: string) => void;
}) {
  return (
    <>
      {nodes.map((node) => (
        <div key={node.path}>
          {node.isFile ? (
            <button
              type="button"
              className={`conversation-list-item vault-tree-item${
                selectedPath === node.path ? " conversation-list-item--active" : ""
              }`}
              style={{ paddingLeft: `${0.65 + depth * 0.9}em` }}
              onClick={() => onSelect(node.path)}
            >
              {node.name}
            </button>
          ) : (
            <div className="vault-tree-folder" style={{ paddingLeft: `${0.65 + depth * 0.9}em` }}>
              {node.name}
            </div>
          )}
          {node.children.length > 0 && (
            <VaultTree nodes={node.children} depth={depth + 1} selectedPath={selectedPath} onSelect={onSelect} />
          )}
        </div>
      ))}
    </>
  );
}

/** The right-hand pane: nothing, loading, failed, a note (with `draft` while editing), or a new note. */
type Open =
  | { kind: "none" }
  | { kind: "loading"; path: string }
  | { kind: "error"; path: string; error: string }
  | { kind: "note"; path: string; note: VaultNote; draft: string | null }
  | { kind: "new"; path: string; draft: string };

function VaultView() {
  const [files, setFiles] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<VaultSearchHit[] | null>(null);
  const [open, setOpen] = useState<Open>({ kind: "none" });
  const [saving, setSaving] = useState(false);
  const [actionError, setActionError] = useState<VaultCmdError | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  function refresh() {
    invoke<string[]>("list_vault_files")
      .then(setFiles)
      .catch((err) => setError(errorOf(err).message));
  }

  // Same "refetch on mount" posture as `UsageView`/`SyncView` — the sidebar remounts this view on
  // each visit, so a file added or edited elsewhere (another channel, an external editor) never
  // shows stale.
  useEffect(refresh, []);

  const dirty = (open.kind === "note" && open.draft !== null && open.draft !== open.note.content) || (open.kind === "new" && open.draft !== "");

  function leaveOk(): boolean {
    return !dirty || window.confirm("Discard unsaved changes?");
  }

  function selectFile(path: string, force = false) {
    if (!force && !leaveOk()) return;
    setActionError(null);
    setConfirmDelete(false);
    setOpen({ kind: "loading", path });
    invoke<VaultNote>("read_vault_note", { path })
      .then((note) => setOpen((cur) => (cur.kind === "loading" && cur.path === path ? { kind: "note", path, note, draft: null } : cur)))
      .catch((err) => setOpen((cur) => (cur.kind === "loading" && cur.path === path ? { kind: "error", path, error: errorOf(err).message } : cur)));
  }

  function startNew() {
    if (!leaveOk()) return;
    setActionError(null);
    setConfirmDelete(false);
    setOpen({ kind: "new", path: "", draft: "" });
  }

  async function save(overwrite = false) {
    if ((open.kind !== "note" && open.kind !== "new") || open.draft === null) return;
    const content = open.draft;
    const path = open.kind === "new" ? normalizeNewPath(open.path) : open.path;
    if (open.kind === "new" && path === ".md") {
      setActionError({ message: "Give the note a name.", conflict: false });
      return;
    }
    setSaving(true);
    setActionError(null);
    try {
      let expectedVersion = open.kind === "note" ? open.note.version : null;
      // Overwriting after a conflict: save over whatever is on disk now.
      if (overwrite) expectedVersion = (await invoke<VaultNote>("read_vault_note", { path })).version;
      const version = await invoke<string>("save_vault_note", { path, content, expectedVersion });
      setOpen({ kind: "note", path, note: { content, version }, draft: null });
      if (open.kind === "new") refresh();
    } catch (err) {
      setActionError(errorOf(err));
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (open.kind !== "note") return;
    setSaving(true);
    setActionError(null);
    try {
      await invoke("delete_vault_note", { path: open.path, expectedVersion: open.note.version });
      setConfirmDelete(false);
      setOpen({ kind: "none" });
      refresh();
    } catch (err) {
      setConfirmDelete(false);
      setActionError(errorOf(err));
    } finally {
      setSaving(false);
    }
  }

  function search(event: FormEvent) {
    event.preventDefault();
    const text = query.trim();
    if (text === "") {
      setHits(null);
      return;
    }
    invoke<VaultSearchHit[]>("search_vault", { query: text })
      .then(setHits)
      .catch((err) => setError(errorOf(err).message));
  }

  if (error) {
    return (
      <div className="settings-view">
        <h2 className="settings-title">Vault</h2>
        <p className="usage-error">{error}</p>
      </div>
    );
  }

  if (!files) {
    return (
      <div className="settings-view">
        <p>Loading vault…</p>
      </div>
    );
  }

  const tree = buildTree(files);
  const selectedPath = open.kind === "none" || open.kind === "new" ? null : open.path;
  const isFixedFile = FIXED_FILES.some((f) => f.file === selectedPath);
  const isMarkdown = selectedPath?.toLowerCase().endsWith(".md") ?? false;

  return (
    <div className="vault-view">
      <h2 className="settings-title">Vault</h2>
      <p className="settings-hint">
        Browse and edit your memory vault. It's a plain, Obsidian-compatible folder, so any text editor works too. A save
        is refused if the note changed since you opened it (e.g. the AI wrote to it).
      </p>

      <div className="vault-body">
        <nav className="vault-nav">
          <form onSubmit={search}>
            <input
              className="settings-input vault-search"
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search the vault"
              aria-label="Search the vault"
            />
          </form>
          <button type="button" className="settings-browse-btn vault-new-btn" onClick={startNew}>
            + New note
          </button>

          {hits !== null ? (
            <>
              <div className="vault-nav-section-label vault-nav-section-row">
                <span>
                  {hits.length} result{hits.length === 1 ? "" : "s"}
                </span>
                <button
                  type="button"
                  className="vault-link-btn"
                  onClick={() => {
                    setHits(null);
                    setQuery("");
                  }}
                >
                  Clear
                </button>
              </div>
              {hits.length === 0 && <p className="settings-hint">Nothing found. Search matches words of 3+ letters.</p>}
              {hits.map((hit) => (
                <button
                  key={`${hit.path}:${hit.lineNumber}`}
                  type="button"
                  className={`conversation-list-item vault-hit${selectedPath === hit.path ? " conversation-list-item--active" : ""}`}
                  onClick={() => selectFile(hit.path)}
                >
                  <span className="vault-hit-path">
                    {hit.path}:{hit.lineNumber}
                  </span>
                  <span className="vault-hit-line">{hit.line.trim()}</span>
                </button>
              ))}
            </>
          ) : (
            <>
              <div className="vault-nav-section-label">Fixed memory</div>
              {FIXED_FILES.map(({ file, label }) => (
                <button
                  key={file}
                  type="button"
                  className={`conversation-list-item${selectedPath === file ? " conversation-list-item--active" : ""}`}
                  onClick={() => selectFile(file)}
                >
                  {label}
                </button>
              ))}

              {tree.length > 0 && (
                <>
                  <div className="vault-nav-section-label">Vault</div>
                  <VaultTree nodes={tree} depth={0} selectedPath={selectedPath} onSelect={selectFile} />
                </>
              )}
            </>
          )}
        </nav>

        <div className="vault-content">
          {(open.kind === "note" || open.kind === "new") && (
            <div className="vault-content-header">
              <span className="vault-content-title">
                {open.kind === "new" ? "New note" : (FIXED_FILES.find((f) => f.file === open.path)?.label ?? open.path)}
              </span>
              {open.kind === "note" && open.draft === null && (
                <span className="vault-content-actions">
                  <button type="button" className="settings-browse-btn" onClick={() => setOpen({ ...open, draft: open.note.content })}>
                    Edit
                  </button>
                  {!isFixedFile &&
                    (confirmDelete ? (
                      <>
                        <button type="button" className="provider-delete-btn" disabled={saving} onClick={() => void remove()}>
                          Really delete
                        </button>
                        <button type="button" className="settings-browse-btn" onClick={() => setConfirmDelete(false)}>
                          Keep
                        </button>
                      </>
                    ) : (
                      <button type="button" className="provider-delete-btn" onClick={() => setConfirmDelete(true)}>
                        Delete
                      </button>
                    ))}
                </span>
              )}
            </div>
          )}

          {actionError && (
            <div className="vault-content-message">
              <p className="settings-error-banner">{actionError.message}</p>
              {actionError.conflict && selectedPath && (
                <span className="vault-content-actions">
                  <button type="button" className="settings-browse-btn" onClick={() => selectFile(selectedPath, true)}>
                    Reload (discards your changes)
                  </button>
                  {open.kind === "note" && open.draft !== null && (
                    <button type="button" className="provider-delete-btn" disabled={saving} onClick={() => void save(true)}>
                      Overwrite
                    </button>
                  )}
                </span>
              )}
            </div>
          )}

          {open.kind === "none" ? (
            <div className="vault-placeholder">
              <p>Select a file to view or edit it.</p>
            </div>
          ) : open.kind === "loading" ? (
            <p className="settings-hint">Loading…</p>
          ) : open.kind === "error" ? (
            <p className="usage-error">{open.error}</p>
          ) : open.kind === "new" || open.draft !== null ? (
            <form
              className="vault-editor"
              onSubmit={(e) => {
                e.preventDefault();
                void save();
              }}
            >
              {open.kind === "new" && (
                <label className="settings-field">
                  <span className="settings-label">Path</span>
                  <input
                    className="settings-input"
                    value={open.path}
                    onChange={(e) => setOpen({ ...open, path: e.target.value })}
                    placeholder="folder/note-name"
                    required
                    autoFocus
                  />
                  <span className="settings-hint">Use / for folders. Without an extension it becomes .md.</span>
                </label>
              )}
              <textarea
                className="settings-input vault-editor-textarea"
                value={open.draft ?? ""}
                onChange={(e) => setOpen({ ...open, draft: e.target.value })}
                placeholder="Write in markdown."
                aria-label="Note content"
                autoFocus={open.kind === "note"}
              />
              <span className="vault-content-actions">
                <button type="submit" className="settings-save-btn" disabled={saving}>
                  {saving ? "Saving…" : "Save"}
                </button>
                <button
                  type="button"
                  className="settings-browse-btn"
                  disabled={saving}
                  onClick={() => {
                    if (!leaveOk()) return;
                    setActionError(null);
                    setOpen(open.kind === "note" ? { ...open, draft: null } : { kind: "none" });
                  }}
                >
                  Cancel
                </button>
              </span>
            </form>
          ) : open.note.content.trim() === "" ? (
            <div className="vault-placeholder">
              <p>Empty — nothing written here yet.</p>
            </div>
          ) : isMarkdown ? (
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
              {open.note.content}
            </ReactMarkdown>
          ) : (
            <pre className="vault-plain">{open.note.content}</pre>
          )}
        </div>
      </div>
    </div>
  );
}

export default VaultView;
