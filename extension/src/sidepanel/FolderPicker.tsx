import { useEffect, useState } from "react";
import type { DirListing } from "../protocol/messages";
import type { ListDirsResponse } from "../background/popup_protocol";

interface Props {
  /** Where the browser opens: the conversation's current choice, or the start. */
  initialPath: string | null;
  onPick: (path: string) => void;
  onCancel: () => void;
}

/** Browses the folders of the hub's machine to choose the folder a conversation works in (P102): the hub lists folders
 * only, and a member sees just the ones the owner allowed them (their folders on nodes included), so at the top their
 * list has no path to pick. Inline, because the side panel is narrow. Talks to the background like `SkillsView`. */
export default function FolderPicker({ initialPath, onPick, onCancel }: Props) {
  const [listing, setListing] = useState<DirListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  async function ask(path?: string): Promise<DirListing | null> {
    const res = (await chrome.runtime.sendMessage({ type: "listDirs", ...(path ? { path } : {}) })) as ListDirsResponse;
    if (!res.ok || !res.listing) {
      setError(res.error ?? "não foi possível listar as pastas");
      return null;
    }
    return res.listing;
  }

  async function open(path?: string) {
    setLoading(true);
    setError(null);
    const next = await ask(path);
    if (next) setListing(next);
    setLoading(false);
  }

  // Opens once. A remembered folder that is gone falls back to the start instead of staying on an error.
  useEffect(() => {
    void (async () => {
      let first = await ask(initialPath ?? undefined);
      if (!first && initialPath) {
        setError(null);
        first = await ask();
      }
      if (first) setListing(first);
      setLoading(false);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const here = listing?.path ?? "";
  return (
    <div className="folder-picker" role="dialog" aria-label="Escolher a pasta de trabalho">
      <p className="folder-picker-hint">A IA lê e escreve nessa pasta e o shell começa nela (cada comando pede o seu sim). Vale para a conversa toda e não muda depois.</p>
      <div className="folder-picker-here" title={here}>
        {here || "Suas pastas"}
      </div>
      {error && <p className="error-banner">{error}</p>}
      <ul className="folder-picker-list" aria-busy={loading}>
        {listing?.parent !== undefined && (
          <li>
            <button type="button" className="folder-picker-item" onClick={() => void open(listing.parent || undefined)}>
              ↑ Subir
            </button>
          </li>
        )}
        {listing?.dirs.map((dir) => (
          <li key={dir.path}>
            <button type="button" className="folder-picker-item" onClick={() => void open(dir.path)}>
              {dir.name}
            </button>
          </li>
        ))}
        {listing && listing.dirs.length === 0 && !loading && <li className="folder-picker-empty">Nenhuma subpasta.</li>}
      </ul>
      <div className="folder-picker-actions">
        <button type="button" className="link-button" onClick={onCancel}>
          Cancelar
        </button>
        <button type="button" disabled={!here || loading} onClick={() => onPick(here)}>
          Usar esta pasta
        </button>
      </div>
    </div>
  );
}
