import { useState } from "react";
import { UserError, type ServerConnection } from "../hub/connection";
import type { UserInfo } from "../hub/messages";

// Recovery (P84 fatia 4 parte B): who besides a person may open their encrypted data. `private`: nobody.
// `consent`: the owner's recovery key and the person's recovery code, together. `company`: the owner
// alone, recorded and told to the person. The owner's key is shown once, when it's made, and typed in
// for each recovery — the hub only keeps its public half. Every change asks for the pairing key.

const POLICIES: Array<{ id: string; name: string; text: string }> = [
  { id: "private", name: "Privado", text: "Ninguém além da pessoa abre os dados dela (senha ou código). Se ela perder os dois, perdeu." },
  { id: "consent", name: "Com consentimento", text: "Você só recupera com a sua chave de recuperação e o código da pessoa, juntos. Nenhum dos dois abre sozinho." },
  { id: "company", name: "De empresa", text: "Você recupera sozinho, com a sua chave. Cada uso fica registrado e a pessoa é avisada ao entrar." },
];

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function when(atMs: number): string {
  return new Date(atMs).toLocaleString("pt-BR");
}

interface Props {
  conn: ServerConnection | null;
  users: UserInfo[];
  /** The workspace's policy now. */
  policy: string;
  /** The lists changed (a policy set, a member recovered). `users` is `null` when only the policy changed. */
  onChanged: (users: UserInfo[] | null, policy: string, tempPassword?: { id: string; password: string }) => void;
}

export default function RecoveryPolicySection({ conn, users, policy, onChanged }: Props) {
  const [chosen, setChosen] = useState(policy);
  const [newKey, setNewKey] = useState(false);
  const [pairingKey, setPairingKey] = useState("");
  const [recovering, setRecovering] = useState<{ user: UserInfo; recoveryKey: string; code: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** The owner's recovery key, shown once. */
  const [secret, setSecret] = useState<string | null>(null);

  const encrypted = users.filter((u) => u.encrypted);

  async function apply(e: React.FormEvent) {
    e.preventDefault();
    if (!conn || busy) return;
    setBusy(true);
    setError(null);
    try {
      const reply = await conn.setRecoveryPolicy(pairingKey, chosen, newKey);
      setSecret(reply.secret ?? null);
      setNewKey(false);
      setPairingKey("");
      onChanged(null, reply.policy);
    } catch (err) {
      setError(err instanceof UserError && err.authRejected ? "Chave de pareamento errada." : message(err));
    } finally {
      setBusy(false);
    }
  }

  async function recover(e: React.FormEvent) {
    e.preventDefault();
    if (!conn || !recovering || busy) return;
    setBusy(true);
    setError(null);
    try {
      const reply = await conn.recoverMember(pairingKey, recovering.user.id, recovering.recoveryKey, recovering.code.trim() || undefined);
      setPairingKey("");
      const id = recovering.user.id;
      setRecovering(null);
      onChanged(reply.users, reply.recoveryPolicy ?? policy, reply.tempPassword ? { id, password: reply.tempPassword } : undefined);
    } catch (err) {
      setError(err instanceof UserError && err.authRejected ? "Chave de pareamento errada." : message(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div className="skills-header">
        <h2>Recuperação dos dados</h2>
      </div>
      <p className="skills-hint">
        Os dados de cada pessoa ficam criptografados no hub. Aqui você decide se pode ajudar quem perdeu a senha e o código. Quem controla a máquina do hub sempre
        consegue, tecnicamente, ver o que um agente vê enquanto trabalha, e o registro e o aviso protegem contra o uso descuidado, não contra quem edita os arquivos
        do hub.
      </p>

      {secret && (
        <div className="error-banner">
          <p>
            <strong>A sua chave de recuperação — aparece só agora.</strong> Anote num lugar seguro, fora desta máquina. Ela nunca fica guardada no hub, e sem ela você não
            consegue recuperar os dados de ninguém.
          </p>
          <p className="recovery-code">
            <code>{secret}</code>
          </p>
          <button type="button" className="link-button" onClick={() => setSecret(null)}>
            Anotei, pode esconder
          </button>
        </div>
      )}

      <form className="settings-confirm devices-confirm" onSubmit={(e) => void apply(e)}>
        <fieldset className="settings-field">
          <legend>Política do workspace</legend>
          {POLICIES.map((p) => (
            <label key={p.id} className="checkbox-row">
              <input type="radio" name="recovery-policy" checked={chosen === p.id} onChange={() => setChosen(p.id)} />
              <span>
                <strong>{p.name}</strong> — {p.text}
              </span>
            </label>
          ))}
          {chosen !== policy && chosen !== "private" && (
            <span className="field-hint">Cada pessoa vê o aviso ao entrar, e uma mudança para uma política mais fraca precisa do aceite dela; até lá os dados continuam como estavam.</span>
          )}
        </fieldset>
        {chosen !== "private" && (
          <label className="checkbox-row">
            <input type="checkbox" checked={newKey} onChange={(e) => setNewKey(e.target.checked)} />
            Trocar a minha chave de recuperação por uma nova
          </label>
        )}
        <label className="settings-field">
          Chave de pareamento do hub
          <input type="password" autoComplete="current-password" value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        </label>
        {error && !recovering && <p className="error-banner">{error}</p>}
        <div className="skills-actions">
          <button type="submit" className="primary-button" disabled={busy || !conn || pairingKey.trim() === "" || (chosen === policy && !newKey)}>
            {busy ? "Aguarde…" : "Aplicar"}
          </button>
        </div>
      </form>

      {policy !== "private" && encrypted.length > 0 && (
        <>
          <h3>Recuperar os dados de uma pessoa</h3>
          <ul className="skills-list">
            {encrypted.map((user) => {
              const mine = recovering?.user.id === user.id ? recovering : null;
              const can = user.memberPolicy === "consent" || user.memberPolicy === "company";
              return (
                <li key={user.id} className="skills-item">
                  <div className="skills-item-header">
                    <span className="skills-item-name">{user.name}</span>
                    <span className="devices-status devices-status--approved">{user.policyPending ? "política pendente" : (user.memberPolicy ?? "")}</span>
                  </div>
                  <p className="skills-item-description">
                    {can
                      ? user.memberPolicy === "consent"
                        ? "Precisa da sua chave e do código de recuperação da pessoa."
                        : "Precisa só da sua chave."
                      : "Os dados ainda estão como “privado”: a pessoa precisa entrar e aceitar a política nova."}
                    {user.recoveries && user.recoveries.length > 0 && ` Recuperado ${user.recoveries.length} vez(es), a última em ${when(user.recoveries[user.recoveries.length - 1].atMs)}.`}
                  </p>
                  {mine ? (
                    <form className="settings-confirm devices-confirm" onSubmit={(e) => void recover(e)}>
                      <label className="settings-field">
                        Sua chave de recuperação
                        <input type="text" autoComplete="off" spellCheck={false} value={mine.recoveryKey} onChange={(e) => setRecovering({ ...mine, recoveryKey: e.target.value })} />
                      </label>
                      {user.memberPolicy === "consent" && (
                        <label className="settings-field">
                          Código de recuperação da pessoa
                          <input type="text" autoComplete="off" spellCheck={false} value={mine.code} onChange={(e) => setRecovering({ ...mine, code: e.target.value })} />
                        </label>
                      )}
                      <label className="settings-field">
                        Chave de pareamento do hub
                        <input type="password" autoComplete="current-password" value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
                      </label>
                      <p className="field-hint">A pessoa recebe uma senha provisória, vê o aviso ao entrar e a recuperação fica registrada.</p>
                      {error && <p className="error-banner">{error}</p>}
                      <div className="skills-actions">
                        <button
                          type="submit"
                          className="primary-button"
                          disabled={busy || pairingKey.trim() === "" || mine.recoveryKey.trim() === "" || (user.memberPolicy === "consent" && mine.code.trim() === "")}
                        >
                          {busy ? "Aguarde…" : "Recuperar"}
                        </button>
                        <button type="button" className="link-button" disabled={busy} onClick={() => setRecovering(null)}>
                          Cancelar
                        </button>
                      </div>
                    </form>
                  ) : (
                    <div className="skills-actions">
                      <button
                        type="button"
                        className="link-button"
                        disabled={!conn || !can || recovering !== null}
                        onClick={() => {
                          setError(null);
                          setRecovering({ user, recoveryKey: "", code: "" });
                        }}
                      >
                        Recuperar
                      </button>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </>
      )}
    </>
  );
}
