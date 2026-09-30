import { useState } from "react";
import { UserError, type ServerConnection } from "../hub/connection";
import type { UserInfo } from "../hub/messages";

interface Props {
  conn: ServerConnection;
  user: UserInfo;
  /** They said yes to the workspace's policy. `recoveryCode`: entering or leaving "consent" made a new one. */
  onAccepted: (recoveryCode?: string) => void;
  /** They've seen the recoveries the owner made. */
  onAcked: () => void;
  /** Not now: the notice comes back at the next sign-in. */
  onLater: () => void;
}

const POLICY_TEXT: Record<string, string> = {
  private: "Só você abre os seus dados (com a senha ou o código de recuperação). Nem quem administra o hub consegue.",
  consent: "Quem administra o hub só consegue recuperar os seus dados com a chave de recuperação dele e o seu código de recuperação, juntos. Nenhum dos dois abre sozinho.",
  company: "Quem administra o hub pode recuperar os seus dados sozinho, com a chave de recuperação dele. Cada recuperação fica registrada e você é avisado.",
};

function when(atMs: number): string {
  return new Date(atMs).toLocaleString("pt-BR");
}

/** P84 fatia 4 parte B: the workspace's recovery policy changed to a weaker one and needs their yes, and/or the
 * owner recovered their data. Shown after signing in, before the app. */
export default function RecoveryNoticeView({ conn, user, onAccepted, onAcked, onLater }: Props) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const policy = user.recoveryPolicy ?? "private";
  const unseen = (user.recoveries ?? []).filter((r) => !r.seen);

  async function accept(e: React.FormEvent) {
    e.preventDefault();
    if (busy || !password) return;
    setBusy(true);
    setError(null);
    try {
      const result = await conn.acceptRecoveryPolicy(password);
      onAccepted(result.recoveryCode);
    } catch (err) {
      setError(err instanceof UserError && err.authRejected ? "A senha está errada." : err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function seen() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await conn.ackRecoveryNotices();
      onAcked();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login">
      <div className="login-card">
        <div className="login-brand">
          <img src="/favicon.svg" alt="" width={40} height={40} />
          <h1>Warden</h1>
        </div>

        {unseen.length > 0 && (
          <>
            <p className="login-hint">
              <strong>Os seus dados foram recuperados por quem administra o hub.</strong> Foi feito com a chave de recuperação dele, e você entrou agora com a senha provisória
              que ele lhe deu.
            </p>
            <ul>
              {unseen.map((event) => (
                <li key={event.atMs}>
                  {when(event.atMs)} — recuperação {event.kind === "consent" ? "com consentimento" : "de empresa"}
                </li>
              ))}
            </ul>
            <p className="login-hint">Se não foi você quem pediu, fale com quem administra o hub e troque a sua senha.</p>
            <button type="button" className="primary-button" disabled={busy} onClick={() => void seen()}>
              Entendi
            </button>
          </>
        )}

        {user.policyPending && (
          <form className="login-card-inner" onSubmit={(e) => void accept(e)}>
            <p className="login-hint">
              <strong>Mudou quem pode ajudar a recuperar os seus dados.</strong> {POLICY_TEXT[policy] ?? ""} Hoje eles seguem a regra anterior; só passam a seguir esta se você aceitar.
            </p>
            <p className="login-hint">
              Vale lembrar: quem controla a máquina do hub sempre consegue, tecnicamente, ver o que o agente vê enquanto trabalha, e o registro e o aviso não protegem contra quem
              edita os arquivos do hub.
            </p>
            <label>
              Sua senha, para aceitar
              <input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
            </label>
            {error && <p className="error-banner">{error}</p>}
            <button type="submit" className="primary-button" disabled={busy || !password}>
              {busy ? "Aguarde…" : "Aceitar"}
            </button>
          </form>
        )}

        {error && unseen.length > 0 && !user.policyPending && <p className="error-banner">{error}</p>}
        <button type="button" className="link-button" onClick={onLater}>
          Agora não
        </button>
      </div>
    </div>
  );
}
