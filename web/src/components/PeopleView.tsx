import { useCallback, useEffect, useState } from "react";
import { UserError, type ServerConnection } from "../hub/connection";
import type { UserInfo } from "../hub/messages";

// People (P84): the members of this workspace besides the owner. Each signs in with a username and
// password, and has their own vault and conversations on this hub. Every change asks for the pairing
// key, like approving a device. A provisional password is shown once, right after it's created.

type Asking =
  | { kind: "create"; id: string; name: string }
  | { kind: "rename"; user: UserInfo; name: string }
  | { kind: "reset"; user: UserInfo }
  | { kind: "remove"; user: UserInfo };

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function PeopleView({ conn }: { conn: ServerConnection | null }) {
  const [users, setUsers] = useState<UserInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** The provisional password to hand over, and to whom — shown once. */
  const [shown, setShown] = useState<{ id: string; password: string } | null>(null);

  const load = useCallback(async () => {
    if (!conn) return;
    try {
      setUsers((await conn.listUsers()).users);
      setError(null);
    } catch (err) {
      setError(message(err));
    }
  }, [conn]);

  useEffect(() => {
    void load();
  }, [load]);

  function cancel() {
    setAsking(null);
    setPairingKey("");
    setKeyError(null);
  }

  async function confirm() {
    if (!conn || !asking) return;
    setBusy(true);
    setKeyError(null);
    try {
      const reply =
        asking.kind === "create"
          ? await conn.saveUser(pairingKey, asking.id.trim().toLowerCase(), asking.name.trim(), true)
          : asking.kind === "rename"
            ? await conn.saveUser(pairingKey, asking.user.id, asking.name.trim(), false)
            : asking.kind === "reset"
              ? await conn.resetPassword(pairingKey, asking.user.id)
              : await conn.removeUser(pairingKey, asking.user.id);
      setUsers(reply.users);
      if (reply.tempPassword) {
        setShown({ id: asking.kind === "create" ? asking.id.trim().toLowerCase() : asking.kind === "reset" ? asking.user.id : "", password: reply.tempPassword });
      }
      cancel();
    } catch (err) {
      if (err instanceof UserError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        setKeyError(message(err));
      }
    } finally {
      setBusy(false);
    }
  }

  const keyForm = (label: string, danger = false, extra?: React.ReactNode) => (
    <form
      className="settings-confirm devices-confirm"
      onSubmit={(e) => {
        e.preventDefault();
        void confirm();
      }}
    >
      {extra}
      <label className="settings-field">
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        <span className="field-hint">É pedida a cada mudança.</span>
      </label>
      {keyError && <p className="error-banner">{keyError}</p>}
      <div className="skills-actions">
        <button type="submit" className={danger ? "primary-button skills-danger" : "primary-button"} disabled={busy || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : label}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancel}>
          Cancelar
        </button>
      </div>
    </form>
  );

  return (
    <div className="skills-view">
      <div className="skills-header">
        <h2>Pessoas</h2>
        <button type="button" className="primary-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "create", id: "", name: "" })}>
          Adicionar pessoa
        </button>
      </div>
      <p className="skills-hint">
        Quem mais usa este Warden. Cada pessoa entra com o próprio usuário e senha e tem o próprio vault e as próprias conversas, que você não vê.
        Por enquanto ela conversa com os seus agentes usando a memória dela, sem terminal, nós ou integrações suas.
      </p>
      {error && <p className="error-banner">{error}</p>}

      {shown && (
        <div className="settings-confirm">
          <p>
            Senha provisória de <strong>{shown.id}</strong> (só aparece agora): <code>{shown.password}</code>
          </p>
          <p className="skills-hint">Passe para a pessoa junto com o usuário. No primeiro acesso ela troca pela senha dela.</p>
          <button type="button" className="link-button" onClick={() => setShown(null)}>
            Já anotei
          </button>
        </div>
      )}

      {asking?.kind === "create" &&
        keyForm(
          "Criar",
          false,
          <>
            <label className="settings-field">
              Usuário
              <input
                autoCapitalize="none"
                value={asking.id}
                onChange={(e) => setAsking({ ...asking, id: e.target.value })}
                placeholder="ana"
                autoFocus
              />
              <span className="field-hint">Letras minúsculas, números, - ou _. É como a pessoa entra.</span>
            </label>
            <label className="settings-field">
              Nome
              <input value={asking.name} onChange={(e) => setAsking({ ...asking, name: e.target.value })} placeholder="Ana Souza" />
            </label>
          </>,
        )}

      {users === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : users.length === 0 ? (
        <p className="skills-hint">Só você por enquanto.</p>
      ) : (
        <ul className="skills-list">
          {users.map((user) => {
            const mine = asking !== null && asking.kind !== "create" && asking.user.id === user.id ? asking : null;
            return (
              <li key={user.id} className="skills-item">
                <div className="skills-item-header">
                  <span className="skills-item-name">{user.name}</span>
                  <span className={`devices-status devices-status--${user.mustChangePassword ? "pending" : "approved"}`}>
                    {user.mustChangePassword ? "Senha provisória" : "Ativo"}
                  </span>
                </div>
                <p className="skills-item-description">
                  <code>{user.id}</code>
                </p>
                {mine?.kind === "rename" &&
                  keyForm(
                    "Salvar",
                    false,
                    <label className="settings-field">
                      Nome
                      <input value={mine.name} onChange={(e) => setAsking({ ...mine, name: e.target.value })} autoFocus />
                    </label>,
                  )}
                {mine?.kind === "reset" && keyForm("Gerar senha provisória")}
                {mine?.kind === "remove" &&
                  keyForm(
                    "Remover",
                    true,
                    <p className="error-banner">
                      {user.name} sai do workspace e os aparelhos dela são desconectados. O vault e as conversas dela ficam guardados no hub.
                    </p>,
                  )}
                {!mine && (
                  <div className="skills-actions">
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "rename", user, name: user.name })}>
                      Renomear
                    </button>
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "reset", user })}>
                      Nova senha provisória
                    </button>
                    <button type="button" className="link-button skills-danger" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "remove", user })}>
                      Remover
                    </button>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
