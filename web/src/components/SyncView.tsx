import { useCallback, useEffect, useState } from "react";
import { SyncError, type ServerConnection } from "../hub/connection";
import type { SyncAction, SyncRound, SyncStatus } from "../hub/messages";

// The hub's vault sync (P61). The agent always works on the hub's own disk; this keeps that disk
// in step with the other devices, every 5 minutes on its own. Here: how it's going, "sync now",
// and — on a hub with no vault key yet — becoming the first device or pairing with one that has
// the key. The git remote itself is set in Settings. Every action asks for the pairing key.

const BACKEND_LABEL: Record<SyncStatus["backend"], string> = {
  notSetUp: "Ainda não configurada",
  git: "Git",
  arweave: "Arweave (TruthID)",
};

const dateFormatter = new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" });

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function roundSummary(round: SyncRound): string {
  const parts: string[] = [];
  if (round.pulled) {
    parts.push(`recebeu ${round.pulled.filesWritten} arquivo(s)${round.pulled.filesDeleted ? `, removeu ${round.pulled.filesDeleted}` : ""}`);
    if (round.pulled.configUpdated) parts.push("configurações atualizadas");
  }
  if (round.pushed) parts.push(`enviou ${round.pushed.filesChanged} arquivo(s)`);
  if (parts.length === 0 && !round.error) parts.push("nada mudou");
  return parts.join(", ");
}

/** What the pairing-key prompt is about to run. */
type Pending = { action: SyncAction; label: string };

export default function SyncView({ conn }: { conn: ServerConnection | null }) {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [code, setCode] = useState("");
  const [host, setHost] = useState("");

  const load = useCallback(async () => {
    if (!conn) return;
    setError(null);
    try {
      setStatus(await conn.requestSyncStatus());
    } catch (err) {
      setError(message(err));
    }
  }, [conn]);

  useEffect(() => {
    void load();
  }, [load]);

  function cancel() {
    setPending(null);
    setPairingKey("");
    setKeyError(null);
  }

  async function confirm() {
    if (!conn || !pending) return;
    setBusy(true);
    setKeyError(null);
    try {
      setStatus(await conn.syncAction(pairingKey, pending.action));
      if (pending.action.kind === "pairJoin") {
        setCode("");
        setHost("");
      }
      cancel();
    } catch (err) {
      if (err instanceof SyncError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        cancel();
        setError(message(err));
      }
    } finally {
      setBusy(false);
    }
  }

  const keyPrompt = pending && (
    <form
      className="settings-confirm devices-confirm"
      onSubmit={(e) => {
        e.preventDefault();
        void confirm();
      }}
    >
      <label className="settings-field">
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        <span className="field-hint">A mesma do primeiro login. É pedida a cada ação.</span>
      </label>
      {keyError && <p className="error-banner">{keyError}</p>}
      {busy && pending.action.kind === "pairJoin" && <p className="skills-hint">Procurando o aparelho que mostra o código…</p>}
      <div className="skills-actions">
        <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : pending.label}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancel}>
          Cancelar
        </button>
      </div>
    </form>
  );

  return (
    <div className="usage-view sync-view">
      <div className="skills-toolbar">
        <span className="skills-hint">
          O agente trabalha no disco deste hub. A sincronização mantém esse disco igual ao dos seus outros aparelhos, sozinha, a
          cada 5 minutos. Tudo sai cifrado.
        </span>
        <button type="button" className="link-button" onClick={() => void load()} disabled={!conn || busy}>
          Recarregar
        </button>
      </div>

      {error && <p className="error-banner">{error}</p>}
      {!status && !error && <p className="skills-hint">Carregando…</p>}

      {status && (
        <section className="usage-section settings-card">
          <h2 className="usage-heading">Destino: {BACKEND_LABEL[status.backend]}</h2>
          {status.gitRemote && (
            <p className="skills-item-description">
              Repositório: <code>{status.gitRemote}</code>
            </p>
          )}
          {status.backend !== "notSetUp" && (
            <p className="skills-item-description">
              {status.pendingVaultChanges === 0 && !status.pendingConfigChanged
                ? "Nada esperando para sair."
                : `Esperando para sair: ${status.pendingVaultChanges} arquivo(s)${status.pendingConfigChanged ? " e as configurações" : ""}.`}{" "}
              {status.lastSyncedAtMs ? `Última sincronização: ${dateFormatter.format(new Date(status.lastSyncedAtMs))}.` : "Nunca sincronizou."}
            </p>
          )}
          {status.lastRound && (
            <p className="skills-item-description">
              Última rodada ({dateFormatter.format(new Date(status.lastRound.atMs))}): {roundSummary(status.lastRound)}
            </p>
          )}
          {status.lastRound?.error && <p className="error-banner">{status.lastRound.error}</p>}
          {status.backend === "arweave" && (
            <p className="skills-hint">
              Com o Arweave o hub só recebe. Enviar pede a aprovação no celular com o TruthID, então é feito pelo desktop ou pelo
              terminal. Para sincronizar nos dois sentidos, configure um repositório git nas Configurações.
            </p>
          )}
          {status.backend !== "notSetUp" &&
            (pending?.action.kind === "syncNow" ? (
              keyPrompt
            ) : (
              <div className="skills-actions">
                <button
                  type="button"
                  className="primary-button"
                  disabled={!conn || pending !== null}
                  onClick={() => setPending({ action: { kind: "syncNow" }, label: "Sincronizar agora" })}
                >
                  Sincronizar agora
                </button>
              </div>
            ))}
        </section>
      )}

      {status?.backend === "notSetUp" && (
        <>
          <section className="usage-section settings-card">
            <h2 className="usage-heading">Parear com outro aparelho</h2>
            <p className="skills-hint">
              Se outro aparelho seu já sincroniza, este hub recebe a chave dele. No outro aparelho, abra a tela de Sync e peça um
              código de pareamento (no terminal: <code>/sync pair</code>).
            </p>
            {pending?.action.kind === "pairJoin" ? (
              keyPrompt
            ) : (
              <form
                className="settings-card"
                onSubmit={(e) => {
                  e.preventDefault();
                  const trimmedHost = host.trim();
                  setPending({ action: { kind: "pairJoin", code: code.trim(), ...(trimmedHost && { host: trimmedHost }) }, label: "Parear" });
                }}
              >
                <label className="settings-field">
                  Código de pareamento
                  <input value={code} onChange={(e) => setCode(e.target.value)} autoComplete="off" />
                </label>
                <label className="settings-field">
                  Endereço do outro aparelho (opcional)
                  <input value={host} onChange={(e) => setHost(e.target.value)} placeholder="100.64.0.2" autoComplete="off" />
                  <span className="field-hint">
                    O IP dele (um do Tailscale serve). Sem isso, o hub só procura na rede local dele — não acha um aparelho de fora.
                  </span>
                </label>
                <div className="skills-actions">
                  <button type="submit" className="primary-button" disabled={!conn || pending !== null || code.trim() === ""}>
                    Continuar
                  </button>
                </div>
              </form>
            )}
          </section>

          <section className="usage-section settings-card">
            <h2 className="usage-heading">Este é o primeiro aparelho</h2>
            <p className="skills-hint">
              Nenhum aparelho seu sincroniza ainda: o hub cria a chave, e os outros pareiam com ele depois. Não faça isso se outro
              aparelho já sincroniza — as memórias ficariam em dois grupos separados.
            </p>
            {pending?.action.kind === "init" ? (
              keyPrompt
            ) : (
              <div className="skills-actions">
                <button type="button" className="link-button" disabled={!conn || pending !== null} onClick={() => setPending({ action: { kind: "init" }, label: "Criar a chave" })}>
                  Criar a chave neste hub
                </button>
              </div>
            )}
          </section>
        </>
      )}
    </div>
  );
}
