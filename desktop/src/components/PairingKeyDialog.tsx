import { useCallback, useRef, useState, type ReactNode } from "react";

/** Asks the person for the hub's pairing key, for one change. Resolves with what was typed. Rejects with `KeyCancelled`
 * when they close the dialog. */
export type AskKey = (reason: string) => Promise<string>;

/** The person closed the pairing key dialog: nothing was asked of the hub, and nothing needs to be shown. */
export class KeyCancelled extends Error {
  constructor() {
    super("cancelled");
    this.name = "KeyCancelled";
  }
}

/** The hub wants its pairing key again for every change to its tasks and webhooks, and this app never keeps the key
 * (P102): it is typed here for the one change and dropped. `dialog` goes anywhere in the screen's JSX. */
export function usePairingKey(): { askKey: AskKey; dialog: ReactNode } {
  const [reason, setReason] = useState<string | null>(null);
  const pending = useRef<{ resolve: (key: string) => void; reject: (error: Error) => void } | null>(null);

  const askKey = useCallback<AskKey>((why) => {
    pending.current?.reject(new KeyCancelled());
    return new Promise<string>((resolve, reject) => {
      pending.current = { resolve, reject };
      setReason(why);
    });
  }, []);

  function finish(key: string | null) {
    const waiting = pending.current;
    pending.current = null;
    setReason(null);
    if (key === null) waiting?.reject(new KeyCancelled());
    else waiting?.resolve(key);
  }

  return { askKey, dialog: reason === null ? null : <PairingKeyDialog reason={reason} onSubmit={(key) => finish(key)} onCancel={() => finish(null)} /> };
}

function PairingKeyDialog({ reason, onSubmit, onCancel }: { reason: string; onSubmit: (key: string) => void; onCancel: () => void }) {
  const [key, setKey] = useState("");

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (key.trim() !== "") onSubmit(key);
  }

  return (
    <div className="settings-modal-backdrop" role="dialog" aria-modal="true" aria-labelledby="pairing-key-title">
      <form className="sync-qr-card settings-modal-card approval-card" onSubmit={submit}>
        <h2 id="pairing-key-title" className="approval-title">
          Pairing key
        </h2>
        <p className="settings-hint">{reason} The hub asks for its pairing key on every change like this one; what you type is used once and not saved.</p>
        <label className="settings-field">
          <span className="settings-label">Pairing key</span>
          <input className="settings-input" type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.currentTarget.value)} autoFocus />
        </label>
        <div className="approval-actions">
          <button type="button" className="settings-browse-btn" onClick={onCancel}>
            Cancel
          </button>
          <button type="submit" className="settings-save-btn" disabled={key.trim() === ""}>
            Continue
          </button>
        </div>
      </form>
    </div>
  );
}
