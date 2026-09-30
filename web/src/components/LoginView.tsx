import { useEffect, useMemo, useState } from "react";
import qrcode from "qrcode-generator";

/** How this browser pairs: the owner with the hub's pairing key, a member (P84) with their username and password. */
export type LoginCredentials = { kind: "key"; authKey: string } | { kind: "user"; username: string; password: string } | { kind: "truthid" };

/** The QR the hub made for the TruthID app (P113), good until `expiresAtMs`. */
export interface TruthIdQr {
  payload: string;
  expiresAtMs: number;
}

interface Props {
  deviceName: string;
  hubAddress: string;
  /** A connection attempt is in flight. */
  busy: boolean;
  /** …and it's using a stored token, not a key typed just now — show "connecting", not the form. */
  resuming: boolean;
  error?: string;
  onSubmit: (credentials: LoginCredentials, deviceName: string) => void;
  /** A TruthID sign-in is waiting for the phone: the QR to scan. */
  truthIdQr?: TruthIdQr | null;
  /** Gives up the TruthID sign-in, or asks for a fresh QR (the app refuses one older than 30 s). */
  onTruthIdCancel?: () => void;
  onTruthIdRefresh?: () => void;
}

/** The QR as an inline SVG, so there's nothing to load and it scales. */
function QrImage({ text }: { text: string }) {
  const svg = useMemo(() => {
    const code = qrcode(0, "M");
    code.addData(text);
    code.make();
    return code.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
  }, [text]);
  return <div className="login-qr" role="img" aria-label="QR para o app TruthID" dangerouslySetInnerHTML={{ __html: svg }} />;
}

function TruthIdWaiting({ qr, onCancel, onRefresh }: { qr: TruthIdQr; onCancel?: () => void; onRefresh?: () => void }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const left = Math.max(0, Math.ceil((qr.expiresAtMs - now) / 1000));
  return (
    <>
      <p className="login-hint">Abra o app TruthID no celular, escaneie o QR e aprove. Vai criar uma sessão na blockchain, então pode levar alguns segundos.</p>
      {left > 0 ? (
        <>
          <QrImage text={qr.payload} />
          <p className="field-hint">O QR vale por mais {left} s.</p>
        </>
      ) : (
        <p className="error-banner">O QR expirou: o app recusa um QR com mais de 30 s.</p>
      )}
      <details>
        <summary>Mostrar o texto do QR</summary>
        <textarea readOnly rows={4} value={qr.payload} onFocus={(e) => e.currentTarget.select()} />
      </details>
      <div className="skills-actions">
        <button type="button" className="primary-button" onClick={onRefresh}>
          Gerar outro QR
        </button>
        <button type="button" className="link-button" onClick={onCancel}>
          Cancelar
        </button>
      </div>
    </>
  );
}

/** Pairs this browser with the hub (P36): the pairing key — or, for a member (P84), a username and
 * password — once, then the hub's device token. */
export default function LoginView({ deviceName, hubAddress, busy, resuming, error, onSubmit, truthIdQr, onTruthIdCancel, onTruthIdRefresh }: Props) {
  const [mode, setMode] = useState<"user" | "key" | "truthid">("user");
  const [authKey, setAuthKey] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState(deviceName);

  const ready = mode === "key" ? authKey.trim() !== "" : mode === "truthid" ? true : username.trim() !== "" && password !== "";

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!ready || busy) return;
    const credentials: LoginCredentials =
      mode === "key" ? { kind: "key", authKey: authKey.trim() } : mode === "truthid" ? { kind: "truthid" } : { kind: "user", username: username.trim(), password };
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
        ) : truthIdQr ? (
          <TruthIdWaiting qr={truthIdQr} onCancel={onTruthIdCancel} onRefresh={onTruthIdRefresh} />
        ) : (
          <>
            <div className="login-modes" role="tablist">
              <button type="button" role="tab" aria-selected={mode === "user"} className={mode === "user" ? "tab tab--active" : "tab"} onClick={() => setMode("user")}>
                Usuário
              </button>
              <button type="button" role="tab" aria-selected={mode === "truthid"} className={mode === "truthid" ? "tab tab--active" : "tab"} onClick={() => setMode("truthid")}>
                TruthID
              </button>
              <button type="button" role="tab" aria-selected={mode === "key"} className={mode === "key" ? "tab tab--active" : "tab"} onClick={() => setMode("key")}>
                Chave de pareamento
              </button>
            </div>
            {mode === "truthid" ? (
              <p className="login-hint">
                Para quem já ligou o TruthID à sua conta (o dono do Warden manda um convite). Você escaneia um QR com o app TruthID, sem senha, e este navegador
                entra como você em <code>{hubAddress}</code>. Os seus dados criptografados continuam pedindo a senha uma vez depois que o hub reinicia.
              </p>
            ) : mode === "user" ? (
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
