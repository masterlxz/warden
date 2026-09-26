import { useCallback, useEffect, useState } from "react";
import { DeviceError, type DeviceList, type ServerConnection } from "../hub/connection";
import type { HubDevice } from "../hub/messages";

// The hub's paired devices (Sessão 103) — the same list, approve and revoke as the desktop's
// Workspace screen and `warden-server devices`, so a hub with no screen can be managed from here.
// Listing is open to any logged-in browser; approving or revoking asks for the pairing key each
// time, like saving settings. Changing the pairing key itself stays on the hub's own machine.

type Action = "approve" | "revoke";

const STATUS_LABEL: Record<HubDevice["status"], string> = {
  pending: "Pendente",
  approved: "Aprovado",
  revoked: "Revogado",
};

const dateFormatter = new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" });

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function DevicesView({ conn }: { conn: ServerConnection | null }) {
  const [list, setList] = useState<DeviceList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [asking, setAsking] = useState<{ device: HubDevice; action: Action } | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!conn) return;
    setError(null);
    try {
      setList(await conn.listDevices());
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
      setList(await conn.setDeviceStatus(pairingKey, asking.device.deviceId, asking.action));
      cancel();
    } catch (err) {
      if (err instanceof DeviceError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        cancel();
        setError(message(err));
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="usage-view devices-view">
      <div className="skills-toolbar">
        <span className="skills-hint">
          Todo aparelho que já se conectou a este hub. Só os aprovados podem usar as ferramentas uns dos outros; um revogado não
          entra mais, nem pareando de novo.
        </span>
        <button type="button" className="link-button" onClick={() => void load()} disabled={!conn || busy}>
          Recarregar
        </button>
      </div>

      {error && <p className="error-banner">{error}</p>}
      {!list && !error && <p className="skills-hint">Carregando…</p>}
      {list && list.devices.length === 0 && <p className="skills-hint">Nenhum aparelho ainda.</p>}

      {list && list.devices.length > 0 && (
        <ul className="skills-list">
          {list.devices.map((device) => {
            const isYou = device.deviceId === list.you;
            const isAsking = asking?.device.deviceId === device.deviceId;
            return (
              <li key={device.deviceId} className="skills-item">
                <div className="skills-item-header">
                  <span className="skills-item-name">
                    {device.deviceName}
                    {isYou && <span className="devices-you"> (este navegador)</span>}
                  </span>
                  <span className={`devices-status devices-status--${device.status}`}>{STATUS_LABEL[device.status]}</span>
                </div>
                <p className="skills-item-description">
                  <code>{device.deviceId}</code> · visto por último em {dateFormatter.format(new Date(device.lastSeenMs))}
                </p>

                {isAsking ? (
                  <form
                    className="settings-confirm devices-confirm"
                    onSubmit={(e) => {
                      e.preventDefault();
                      void confirm();
                    }}
                  >
                    {asking.action === "revoke" && isYou && (
                      <p className="error-banner">Este navegador vai ser desconectado em alguns segundos e não entra mais.</p>
                    )}
                    <label className="settings-field">
                      Chave de pareamento do hub
                      <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
                      <span className="field-hint">A mesma do primeiro login. É pedida a cada mudança.</span>
                    </label>
                    {keyError && <p className="error-banner">{keyError}</p>}
                    <div className="skills-actions">
                      <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
                        {busy ? "Aguarde…" : asking.action === "approve" ? "Aprovar" : "Revogar"}
                      </button>
                      <button type="button" className="link-button" disabled={busy} onClick={cancel}>
                        Cancelar
                      </button>
                    </div>
                  </form>
                ) : (
                  <div className="skills-actions">
                    {device.status !== "approved" && (
                      <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ device, action: "approve" })}>
                        {device.status === "revoked" ? "Aprovar de novo" : "Aprovar"}
                      </button>
                    )}
                    {device.status !== "revoked" && (
                      <button
                        type="button"
                        className="link-button skills-danger"
                        disabled={!conn || asking !== null}
                        onClick={() => setAsking({ device, action: "revoke" })}
                      >
                        Revogar
                      </button>
                    )}
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
