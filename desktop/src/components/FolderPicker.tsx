import { useEffect, useState } from "react";
import { folderPlace, nodeFolderRef, parseNodeFolder, type DirListing, type NodeInfo } from "../lib/workdir";

interface Props {
  /** The folders inside a path of the hub's machine, or of a node when the path is `node:<id>:<path>`. No path starts where
   * the person may. */
  listDirs: (path?: string) => Promise<DirListing>;
  /** The machines other than the hub a folder can be picked on: only the owner has any to choose from. */
  nodes?: NodeInfo[];
  /** Where the browser opens: the conversation's current choice, or the start. */
  initialPath?: string;
  onPick: (path: string) => void;
  onCancel: () => void;
}

/** Browses the folders of a hub (or of a node it has), to choose the folder a conversation works in (P102). The hub lists
 * folders only; a member sees just the ones the owner allowed them, so at the top their list has no path to pick. */
function FolderPicker({ listDirs, nodes = [], initialPath, onPick, onCancel }: Props) {
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

  // Opens once. A remembered folder that is gone falls back to the start instead of an error; when the start itself
  // can't be listed, the hub's own reason is what is shown.
  useEffect(() => {
    let live = true;
    listDirs(initialPath)
      .catch((err) => (initialPath ? listDirs() : Promise.reject(err)))
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
    <div className="settings-modal-backdrop" role="dialog" aria-modal="true" aria-labelledby="folder-picker-title">
      <div className="sync-qr-card settings-modal-card approval-card folder-picker">
        <h2 id="folder-picker-title" className="approval-title">
          Choose the working folder
        </h2>
        <p className="settings-hint">
          The AI reads and writes in this folder, and the shell starts there (each command asks for your yes). It applies to the whole conversation and does not
          change afterwards.
        </p>
        {nodes.length > 0 && (
          <label className="settings-field folder-picker-machine">
            <span className="settings-label">Machine</span>
            <select
              className="settings-input"
              value={machine}
              disabled={loading}
              onChange={(e) => open(e.currentTarget.value ? nodeFolderRef(e.currentTarget.value, "") : undefined)}
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
          {here ? folderPlace(here, nodes) : "Your folders"}
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
                ↑ Up
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
          {listing && listing.dirs.length === 0 && !loading && <li className="folder-picker-empty">No subfolders.</li>}
        </ul>
        <div className="approval-actions">
          <button type="button" className="settings-browse-btn" onClick={onCancel}>
            Cancel
          </button>
          <button type="button" className="settings-save-btn" disabled={!here || loading} onClick={() => onPick(here)}>
            Use this folder
          </button>
        </div>
      </div>
    </div>
  );
}

export default FolderPicker;
