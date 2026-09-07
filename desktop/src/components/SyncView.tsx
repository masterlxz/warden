import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { SyncPullResult, SyncPushBegin, SyncPushResult, SyncStatus } from "../types";

function formatTimestamp(ms: number | null): string {
  if (ms === null) return "nunca";
  return new Date(ms).toLocaleString();
}

function StatusCard({ status }: { status: SyncStatus }) {
  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Status</h3>
      </div>
      <div className="sync-status-grid">
        <div className="sync-status-item">
          <span className="sync-status-label">Pareado</span>
          <span className="sync-status-value">{status.paired ? "Sim" : "Não"}</span>
        </div>
        <div className="sync-status-item">
          <span className="sync-status-label">Endereço do dono (Arweave)</span>
          <span className="sync-status-value sync-status-value--mono">{status.ownerAddress ?? "—"}</span>
        </div>
        <div className="sync-status-item">
          <span className="sync-status-label">Última sincronização</span>
          <span className="sync-status-value">{formatTimestamp(status.lastSyncedAtMs)}</span>
        </div>
        <div className="sync-status-item">
          <span className="sync-status-label">Pendências</span>
          <span className="sync-status-value">
            {status.pendingVaultChanges} arquivo(s){status.pendingConfigChanged ? " + config.toml" : ""}
          </span>
        </div>
      </div>
    </section>
  );
}

function SyncView() {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [pushBegin, setPushBegin] = useState<SyncPushBegin | null>(null);
  const [pushResult, setPushResult] = useState<SyncPushResult | null>(null);
  const [pullResult, setPullResult] = useState<SyncPullResult | null>(null);

  const [pairingCode, setPairingCode] = useState<string | null>(null);
  const [pairingStatus, setPairingStatus] = useState<"idle" | "hosting" | "completed" | "failed">("idle");
  const [joinCode, setJoinCode] = useState("");
  const [joining, setJoining] = useState(false);

  const unlistenRef = useRef<(() => void) | null>(null);

  function refreshStatus() {
    invoke<SyncStatus>("sync_status")
      .then(setStatus)
      .catch((err) => setError(String(err)));
  }

  // Same "refetch on mount" posture as `UsageView`/`SettingsView` — the sidebar remounts this
  // view on each visit, so a stale status right after a change made elsewhere never lingers.
  useEffect(() => {
    refreshStatus();
    return () => {
      unlistenRef.current?.();
    };
  }, []);

  async function handleInit() {
    setBusy(true);
    setError(null);
    try {
      await invoke("sync_init");
      refreshStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleSend() {
    setBusy(true);
    setError(null);
    setPushResult(null);
    try {
      const begin = await invoke<SyncPushBegin>("sync_push_begin");
      setPushBegin(begin);
      const result = await invoke<SyncPushResult>("sync_push_await");
      setPushResult(result);
      setPushBegin(null);
      refreshStatus();
    } catch (err) {
      setError(String(err));
      setPushBegin(null);
    } finally {
      setBusy(false);
    }
  }

  async function handlePull() {
    setBusy(true);
    setError(null);
    setPullResult(null);
    try {
      const result = await invoke<SyncPullResult>("sync_pull");
      setPullResult(result);
      refreshStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleStartPairing() {
    setError(null);
    setPairingStatus("hosting");
    try {
      const { code } = await invoke<{ code: string }>("pairing_start");
      setPairingCode(code);
      unlistenRef.current?.();
      const unlistenCompleted = await listen("pairing-completed", () => {
        setPairingStatus("completed");
        refreshStatus();
      });
      const unlistenFailed = await listen<string>("pairing-failed", (event) => {
        setPairingStatus("failed");
        setError(event.payload);
      });
      unlistenRef.current = () => {
        unlistenCompleted();
        unlistenFailed();
      };
    } catch (err) {
      setError(String(err));
      setPairingStatus("idle");
    }
  }

  async function handleJoin(e: React.FormEvent) {
    e.preventDefault();
    setJoining(true);
    setError(null);
    try {
      await invoke("pairing_join", { code: joinCode.trim() });
      setJoinCode("");
      refreshStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setJoining(false);
    }
  }

  if (error && !status) {
    return (
      <div className="settings-view">
        <h2 className="settings-title">Sync</h2>
        <p className="settings-error-banner">{error}</p>
      </div>
    );
  }

  if (!status) {
    return (
      <div className="settings-view">
        <p>Carregando sync…</p>
      </div>
    );
  }

  return (
    <div className="settings-view">
      <h2 className="settings-title">Sync</h2>
      <p className="settings-hint">
        Sincroniza o vault e o config.toml entre seus dispositivos via Arweave, pago pelo app TruthID. Tudo é cifrado
        neste dispositivo antes de sair — nem o TruthID nem o Arweave veem o conteúdo em texto puro.
      </p>

      {error && <p className="settings-error-banner">{error}</p>}

      <StatusCard status={status} />

      {!status.paired ? (
        <section className="settings-section">
          <div className="settings-section-header">
            <h3 className="settings-section-title">Configurar</h3>
          </div>
          <p className="settings-hint">
            Este é o primeiro dispositivo? Inicialize uma chave nova. Já tem outro dispositivo com sync configurado?
            Peça pra ele mostrar um código de pareamento (abaixo).
          </p>
          <button type="button" className="settings-save-btn" onClick={handleInit} disabled={busy}>
            Inicializar sync neste dispositivo
          </button>
        </section>
      ) : (
        <section className="settings-section">
          <div className="settings-section-header">
            <h3 className="settings-section-title">Enviar / Pull</h3>
          </div>
          <div className="sync-actions-row">
            <button type="button" className="settings-save-btn" onClick={handleSend} disabled={busy}>
              Enviar
            </button>
            <button type="button" className="settings-save-btn" onClick={handlePull} disabled={busy}>
              Pull
            </button>
          </div>

          {pushBegin && (
            <div className="sync-qr-card">
              <p className="settings-hint">
                Escaneie com o app TruthID pra aprovar e publicar ({pushBegin.filesChanged} arquivo(s)
                {pushBegin.configChanged ? " + config.toml" : ""}).
              </p>
              <div className="sync-qr-image" dangerouslySetInnerHTML={{ __html: pushBegin.qrSvg }} />
              <p className="settings-hint">Aguardando aprovação…</p>
            </div>
          )}

          {pushResult && (
            <p className="settings-success-banner">
              Enviado — tx {pushResult.txId} ({pushResult.filesChanged} arquivo(s))
            </p>
          )}

          {pullResult && (
            <div className="settings-success-banner">
              <p>
                Pull concluído — {pullResult.filesWritten} escrito(s), {pullResult.filesDeleted} removido(s)
                {pullResult.configUpdated ? ", config.toml atualizado" : ""}.
              </p>
              {pullResult.warnings.length > 0 && (
                <ul className="sync-warning-list">
                  {pullResult.warnings.map((w) => (
                    <li key={w}>{w}</li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </section>
      )}

      <section className="settings-section">
        <div className="settings-section-header">
          <h3 className="settings-section-title">Parear novo dispositivo</h3>
        </div>

        {status.paired && (
          <div className="sync-pairing-block">
            <p className="settings-hint">Mostre este código no dispositivo que quer parear.</p>
            {pairingStatus === "idle" ? (
              <button type="button" className="settings-browse-btn" onClick={handleStartPairing}>
                Mostrar código de pareamento
              </button>
            ) : (
              <>
                {pairingCode && <p className="sync-pairing-code">{pairingCode}</p>}
                {pairingStatus === "hosting" && <p className="settings-hint">Aguardando outro dispositivo…</p>}
                {pairingStatus === "completed" && <p className="settings-success-banner">Pareado com sucesso!</p>}
              </>
            )}
          </div>
        )}

        <div className="sync-pairing-block">
          <p className="settings-hint">Tem um código mostrado em outro dispositivo? Digite-o aqui.</p>
          <form className="sync-join-form" onSubmit={handleJoin}>
            <input
              type="text"
              className="settings-input"
              placeholder="XXXXXXXX"
              value={joinCode}
              onChange={(e) => setJoinCode(e.target.value)}
              disabled={joining}
            />
            <button type="submit" className="settings-save-btn" disabled={joining || joinCode.trim().length === 0}>
              Parear
            </button>
          </form>
        </div>
      </section>
    </div>
  );
}

export default SyncView;
