import { useState } from "react";
import { UserError, type ServerConnection } from "../hub/connection";

const MIN_PASSWORD_LEN = 8;

interface Props {
  conn: ServerConnection;
  /** How the member is shown. */
  name: string;
  /** `true` on the provisional password: nothing else works until it's changed, so there's no way back. */
  required: boolean;
  onDone: () => void;
  onCancel?: () => void;
  onLogout: () => void;
}

/** P84: a member swaps the provisional password the owner gave them for their own — required on the
 * first sign-in, and available later from the header. */
export default function ChangePasswordView({ conn, name, required, onDone, onCancel, onLogout }: Props) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const problem =
    next.length > 0 && next.length < MIN_PASSWORD_LEN
      ? `A senha nova precisa de pelo menos ${MIN_PASSWORD_LEN} caracteres.`
      : again.length > 0 && again !== next
        ? "As duas senhas novas não são iguais."
        : null;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (busy || problem || !current || !next || next !== again) return;
    setBusy(true);
    setError(null);
    try {
      await conn.changePassword(current, next);
      onDone();
    } catch (err) {
      setError(err instanceof UserError && err.authRejected ? "A senha atual está errada." : err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login">
      <form className="login-card" onSubmit={(e) => void handleSubmit(e)}>
        <div className="login-brand">
          <img src="/favicon.svg" alt="" width={40} height={40} />
          <h1>Warden</h1>
        </div>
        <p className="login-hint">
          {required ? `Olá, ${name}. Antes de começar, troque a senha provisória por uma sua.` : "Troque a sua senha."}
        </p>
        <label>
          {required ? "Senha provisória" : "Senha atual"}
          <input type="password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} autoFocus />
        </label>
        <label>
          Senha nova
          <input type="password" autoComplete="new-password" value={next} onChange={(e) => setNext(e.target.value)} />
        </label>
        <label>
          Repita a senha nova
          <input type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} />
        </label>
        {(problem || error) && <p className="error-banner">{problem ?? error}</p>}
        <button type="submit" className="primary-button" disabled={busy || problem !== null || !current || !next || next !== again}>
          {busy ? "Aguarde…" : "Trocar senha"}
        </button>
        {onCancel && !required ? (
          <button type="button" className="link-button" onClick={onCancel}>
            Cancelar
          </button>
        ) : (
          <button type="button" className="link-button" onClick={onLogout}>
            Sair
          </button>
        )}
      </form>
    </div>
  );
}
