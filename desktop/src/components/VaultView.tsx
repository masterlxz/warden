import { useEffect, useState } from "react";
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

function VaultView() {
  const [files, setFiles] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [contentLoading, setContentLoading] = useState(false);
  const [contentError, setContentError] = useState<string | null>(null);

  // Same "refetch on mount" posture as `UsageView`/`SyncView` — the sidebar remounts this view on
  // each visit, so a file added or edited elsewhere (another channel, an external editor) never
  // shows stale.
  useEffect(() => {
    invoke<string[]>("list_vault_files")
      .then(setFiles)
      .catch((err) => setError(String(err)));
  }, []);

  function selectFile(path: string) {
    setSelectedPath(path);
    setContent(null);
    setContentError(null);
    setContentLoading(true);
    invoke<string>("read_vault_file", { relativePath: path })
      .then(setContent)
      .catch((err) => setContentError(String(err)))
      .finally(() => setContentLoading(false));
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
  const isFixedFile = FIXED_FILES.some((f) => f.file === selectedPath);

  return (
    <div className="vault-view">
      <h2 className="settings-title">Vault</h2>
      <p className="settings-hint">
        Browse your memory vault. Read-only for now — edit files with any text editor (it's a plain, Obsidian-compatible
        folder), or let the AI write to them.
      </p>

      <div className="vault-body">
        <nav className="vault-nav">
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
        </nav>

        <div className="vault-content">
          {!selectedPath ? (
            <div className="vault-placeholder">
              <p>Select a file to view its content.</p>
            </div>
          ) : contentLoading ? (
            <p className="settings-hint">Loading…</p>
          ) : contentError ? (
            isFixedFile ? (
              <div className="vault-placeholder">
                <p>Empty — nothing written here yet.</p>
              </div>
            ) : (
              <p className="usage-error">{contentError}</p>
            )
          ) : (
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
              {content ?? ""}
            </ReactMarkdown>
          )}
        </div>
      </div>
    </div>
  );
}

export default VaultView;
