import { useEffect, useState } from "react";
import type { DirListing, NodeInfo } from "../hub/messages";
import { folderPlace, nodeFolderRef, parseNodeFolder } from "../hub/workdir";

interface Props {
  /** The folders inside a path (P102): of the hub's machine, or of a node when the path is `node:<id>:<path>`. No path
   * starts where the person may. */
  listDirs: (path?: string) => Promise<DirListing>;
  /** The machines other than the hub a folder can be picked on (fatia 2): only the owner has any to choose from. */
  nodes?: NodeInfo[];
  /** Where the browser opens: the conversation's current choice, or the start. */
  initialPath?: string;
  onPick: (path: string) => void;
  onCancel: () => void;
}

/** Browses the folders of the hub's machine, or of a node, to choose the folder a conversation works in (P102). The hub
 * lists folders only; a member sees just the ones the owner allowed them, so at the top their list has no path to pick. */
export default function FolderPicker({ listDirs, nodes = [], initialPath, onPick, onCancel }: Props) {
  const [listing, setListing] = useState<DirListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  function open(path?: string) {
    setLoading(true);
    setError(null);
    listDirs(path)
      .then(setListing)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoading(false));
  }

  // Opens once. A remembered folder that is gone falls back to the start instead of an error.
  useEffect(() => {
    let live = true;
    listDirs(initialPath)
      .catch(() => (initialPath ? listDirs() : Promise.reject(new Error("não foi possível listar as pastas"))))
      .then((result) => live && setListing(result))
      .catch((err) => live && setError(err instanceof Error ? err.message : String(err)))
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const here = listing?.path ?? "";
  // Which machine the list is of: the hub, or the node the path names.
  const machine = parseNodeFolder(here)?.node ?? "";
  return (
    <div className="approval-backdrop" role="dialog" aria-modal="true" aria-labelledby="folder-picker-title">
      <div className="approval-card folder-picker">
        <h2 id="folder-picker-title" className="approval-title">
          Escolher a pasta de trabalho
        </h2>
        <p className="approval-more">
          A IA lê e escreve nessa pasta, e o shell começa nela (cada comando pede o seu sim). Vale para a conversa toda e não muda depois.
        </p>
        {nodes.length > 0 && (
          <label className="agent-picker folder-picker-machine">
            <span className="agent-picker-label">Máquina</span>
            <select
              value={machine}
              disabled={loading}
              onChange={(e) => open(e.target.value ? nodeFolderRef(e.target.value, "") : undefined)}
            >
              <option value="">Hub</option>
              {nodes.map((n) => (
                <option key={n.deviceId} value={n.deviceId}>
                  {n.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <div className="folder-picker-here" title={here}>
          {here ? folderPlace(here, nodes) : "Suas pastas"}
        </div>
        {error && (
          <p className="folder-picker-error" role="alert">
            {error}
          </p>
        )}
        <ul className="folder-picker-list" aria-busy={loading}>
          {listing?.parent !== undefined && (
            <li>
              <button type="button" className="folder-picker-item" onClick={() => open(listing.parent || undefined)}>
                ↑ Subir
              </button>
            </li>
          )}
          {listing?.dirs.map((dir) => (
            <li key={dir.path}>
              <button type="button" className="folder-picker-item" onClick={() => open(dir.path)}>
                {dir.name}
              </button>
            </li>
          ))}
          {listing && listing.dirs.length === 0 && !loading && <li className="folder-picker-empty">Nenhuma subpasta.</li>}
        </ul>
        <div className="approval-actions">
          <button type="button" className="link-button" onClick={onCancel}>
            Cancelar
          </button>
          <button type="button" className="primary-button" disabled={!here || loading} onClick={() => onPick(here)}>
            Usar esta pasta
          </button>
        </div>
      </div>
    </div>
  );
}
