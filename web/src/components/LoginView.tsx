import { useState } from "react";

/** How this browser pairs: the owner with the hub's pairing key, a member (P84) with their username and password. */
export type LoginCredentials = { kind: "key"; authKey: string } | { kind: "user"; username: string; password: string };

interface Props {
  deviceName: string;
  hubAddress: string;
  /** A connection attempt is in flight. */
  busy: boolean;
  /** …and it's using a stored token, not a key typed just now — show "connecting", not the form. */
  resuming: boolean;
  error?: string;
  onSubmit: (credentials: LoginCredentials, deviceName: string) => void;
}

/** Pairs this browser with the hub (P36): the pairing key — or, for a member (P84), a username and
 * password — once, then the hub's device token. */
export default function LoginView({ deviceName, hubAddress, busy, resuming, error, onSubmit }: Props) {
  const [mode, setMode] = useState<"user" | "key">("user");
  const [authKey, setAuthKey] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState(deviceName);

  const ready = mode === "key" ? authKey.trim() !== "" : username.trim() !== "" && password !== "";

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!ready || busy) return;
    const credentials: LoginCredentials = mode === "key" ? { kind: "key", authKey: authKey.trim() } : { kind: "user", username: username.trim(), password };
    onSubmit(credentials, name.trim() || deviceName);
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
            <div className="login-modes" role="tablist">
              <button type="button" role="tab" aria-selected={mode === "user"} className={mode === "user" ? "tab tab--active" : "tab"} onClick={() => setMode("user")}>
                Usuário
              </button>
              <button type="button" role="tab" aria-selected={mode === "key"} className={mode === "key" ? "tab tab--active" : "tab"} onClick={() => setMode("key")}>
                Chave de pareamento
              </button>
            </div>
            {mode === "user" ? (
              <>
                <p className="login-hint">
                  Entre no hub em <code>{hubAddress}</code> com o usuário que o dono do Warden criou para você. Na primeira vez, você troca a senha
                  provisória pela sua.
                </p>
                <label>
                  Usuário
                  <input autoComplete="username" autoCapitalize="none" value={username} onChange={(e) => setUsername(e.target.value)} autoFocus />
                </label>
                <label>
                  Senha
                  <input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
                </label>
              </>
            ) : (
              <>
                <p className="login-hint">
                  Para o dono do Warden: conecte este navegador ao hub em <code>{hubAddress}</code>. A chave de pareamento aparece nas configurações
                  do hub (no desktop, em Workspace). Ela só é pedida uma vez.
                </p>
                <label>
                  Chave de pareamento
                  <input type="password" autoComplete="off" value={authKey} onChange={(e) => setAuthKey(e.target.value)} autoFocus />
                </label>
              </>
            )}
            <label>
              Nome deste navegador
              <input value={name} onChange={(e) => setName(e.target.value)} placeholder={deviceName} />
              <span className="field-hint">É como ele aparece na lista de aparelhos do hub.</span>
            </label>
            {error && <p className="error-banner">{error}</p>}
            <button type="submit" className="primary-button" disabled={busy || !ready}>
              {busy ? "Conectando…" : "Entrar"}
            </button>
          </>
        )}
      </form>
    </div>
  );
}
