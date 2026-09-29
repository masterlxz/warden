import { useState } from "react";
import { UserError, type ServerConnection } from "../hub/connection";

const MIN_PASSWORD_LEN = 8;

interface Props {
  conn: ServerConnection;
  /** How the member is shown. */
  name: string;
  /** `true` on the provisional password: nothing else works until it's changed, so there's no way back. */
  required: boolean;
  /** P84 fatia 4: the owner reset the password of someone whose data is encrypted — the recovery code opens it. */
  needsRecovery?: boolean;
  /** Their data is encrypted, so they can ask for a new recovery code. */
  encrypted?: boolean;
  /** `recoveryCode`: this change turned encryption on — it has to be shown to them. */
  onDone: (recoveryCode?: string) => void;
  /** A new code they asked for. */
  onNewCode?: (code: string) => void;
  onCancel?: () => void;
  onLogout: () => void;
}

/** P84: a member swaps the provisional password the owner gave them for their own — required on the
 * first sign-in, and available later from the header. */
export default function ChangePasswordView({ conn, name, required, needsRecovery = false, encrypted = false, onDone, onNewCode, onCancel, onLogout }: Props) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [code, setCode] = useState("");
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
    if (busy || problem || !current || !next || next !== again || (needsRecovery && !code.trim())) return;
    setBusy(true);
    setError(null);
    try {
      const result = await conn.changePassword(current, next, needsRecovery ? code : undefined);
      onDone(result.recoveryCode);
    } catch (err) {
      setError(err instanceof UserError && err.authRejected ? "A senha atual está errada." : err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  /** A new recovery code, with the password typed above; the old code stops working. */
  async function handleNewCode() {
    if (busy || !current) return;
    setBusy(true);
    setError(null);
    try {
      onNewCode?.(await conn.regenerateRecoveryCode(current));
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
        {needsRecovery && (
          <label>
            Código de recuperação
            <input type="text" autoComplete="off" autoCapitalize="characters" spellCheck={false} value={code} onChange={(e) => setCode(e.target.value)} />
            <span className="field-hint">
              Quem administra o hub redefiniu a sua senha. Os seus dados só voltam com o código que você anotou quando eles foram criptografados.
            </span>
          </label>
        )}
        <label>
          Senha nova
          <input type="password" autoComplete="new-password" value={next} onChange={(e) => setNext(e.target.value)} />
        </label>
        <label>
          Repita a senha nova
          <input type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} />
        </label>
        {(problem || error) && <p className="error-banner">{problem ?? error}</p>}
        <button type="submit" className="primary-button" disabled={busy || problem !== null || !current || !next || next !== again || (needsRecovery && !code.trim())}>
          {busy ? "Aguarde…" : "Trocar senha"}
        </button>
        {encrypted && !required && (
          <button type="button" className="link-button" disabled={busy || !current} onClick={() => void handleNewCode()}>
            Gerar um novo código de recuperação (use a senha atual acima)
          </button>
        )}
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
