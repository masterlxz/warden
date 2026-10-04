import { useState } from "react";
import type { SavedHub } from "../types";
import type { HubCredential } from "../lib/hub";

interface Props {
  hub: SavedHub;
  /** Signs in with what was typed; rejects with the reason, which is shown here. */
  onSubmit: (credential: HubCredential) => Promise<void>;
  onCancel: () => void;
}

/** The first sign-in to a hub: its pairing key (the owner) or a username and password (a member of its workspace). What
 * is typed is used once and not kept; the hub hands back a token, and that is what this computer remembers. */
function HubConnectDialog({ hub, onSubmit, onCancel }: Props) {
  const [kind, setKind] = useState<"key" | "member">("key");
  const [key, setKey] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ready = kind === "key" ? key.trim() !== "" : username.trim() !== "" && password !== "";

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!ready || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onSubmit(kind === "key" ? { kind: "key", key } : { kind: "member", username, password });
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  }

  return (
    <div className="settings-modal-backdrop" role="dialog" aria-modal="true" aria-labelledby="hub-connect-title">
      <form className="sync-qr-card settings-modal-card approval-card" onSubmit={submit}>
        <h2 id="hub-connect-title" className="approval-title">
          Sign in to {hub.name}
        </h2>
        <p className="settings-hint">
          {hub.url} — what you type is used once and not saved: the hub gives this computer a token, and that is what is remembered.
        </p>
        <div className="hub-connect-kinds" role="radiogroup" aria-label="How to sign in">
          <label>
            <input type="radio" name="hub-kind" checked={kind === "key"} onChange={() => setKind("key")} /> Pairing key (the owner)
          </label>
          <label>
            <input type="radio" name="hub-kind" checked={kind === "member"} onChange={() => setKind("member")} /> Username and password
          </label>
        </div>
        {kind === "key" ? (
          <label className="settings-field">
            <span className="settings-label">Pairing key</span>
            <input className="settings-input" type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.currentTarget.value)} autoFocus />
          </label>
        ) : (
          <>
            <label className="settings-field">
              <span className="settings-label">Username</span>
              <input className="settings-input" type="text" autoComplete="off" autoCapitalize="none" value={username} onChange={(e) => setUsername(e.currentTarget.value)} autoFocus />
            </label>
            <label className="settings-field">
              <span className="settings-label">Password</span>
              <input className="settings-input" type="password" autoComplete="off" value={password} onChange={(e) => setPassword(e.currentTarget.value)} />
            </label>
          </>
        )}
        {error && <p className="settings-error-banner">{error}</p>}
        <div className="approval-actions">
          <button type="button" className="settings-browse-btn" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="settings-save-btn" disabled={!ready || busy}>
            {busy ? "Connecting…" : "Connect"}
          </button>
        </div>
      </form>
    </div>
  );
}

export default HubConnectDialog;
