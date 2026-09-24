import { useState } from "react";

interface Props {
  deviceName: string;
  hubAddress: string;
  /** A connection attempt is in flight. */
  busy: boolean;
  /** …and it's using a stored token, not a key typed just now — show "connecting", not the form. */
  resuming: boolean;
  error?: string;
  onSubmit: (authKey: string, deviceName: string) => void;
}

/** Pairs this browser with the hub (P36): the pairing key once, then the hub's device token. */
export default function LoginView({ deviceName, hubAddress, busy, resuming, error, onSubmit }: Props) {
  const [authKey, setAuthKey] = useState("");
  const [name, setName] = useState(deviceName);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!authKey.trim() || busy) return;
    onSubmit(authKey.trim(), name.trim() || deviceName);
  }

  return (
    <div className="login">
      <form className="login-card" onSubmit={handleSubmit}>
        <div className="login-brand">
          <img src="/favicon.svg" alt="" width={40} height={40} />
          <h1>Warden</h1>
        </div>
        {resuming ? (
          <p className="login-hint">Conectando ao hub…</p>
        ) : (
          <>
            <p className="login-hint">
              Conecte este navegador ao hub em <code>{hubAddress}</code>. A chave de pareamento aparece nas configurações do hub (no desktop, em
              Workspace). Ela só é pedida uma vez.
            </p>
            <label>
              Chave de pareamento
              <input type="password" autoComplete="off" value={authKey} onChange={(e) => setAuthKey(e.target.value)} autoFocus required />
            </label>
            <label>
              Nome deste navegador
              <input value={name} onChange={(e) => setName(e.target.value)} placeholder={deviceName} />
              <span className="field-hint">É como ele aparece na lista de devices do hub.</span>
            </label>
            {error && <p className="error-banner">{error}</p>}
            <button type="submit" className="primary-button" disabled={busy || !authKey.trim()}>
              {busy ? "Conectando…" : "Conectar"}
            </button>
          </>
        )}
      </form>
    </div>
  );
}
