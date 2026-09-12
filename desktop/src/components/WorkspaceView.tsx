import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { HubPairingConfig, PairedDevice } from "../types";

const dateFormatter = new Intl.DateTimeFormat("en-US", { dateStyle: "medium", timeStyle: "short" });

function formatSeen(ms: number): string {
  return dateFormatter.format(new Date(ms));
}

function StatusBadge({ status }: { status: PairedDevice["status"] }) {
  const label = status === "pending" ? "Pending" : status === "approved" ? "Approved" : "Revoked";
  return <span className={`storage-provider-badge workspace-status-badge workspace-status-badge--${status}`}>{label}</span>;
}

/** Fase 9.7 — lets the operator save the hub's connection details once and generate a QR a new
 * client (today, the mobile app's `ConnectionScreen`) scans instead of typing them by hand.
 * Doesn't touch `PairingStore`/approval at all — a scanned device still shows up as `pending`
 * above like any other, same as one that connected with hand-typed values. */
function HubPairingQrSection() {
  const [config, setConfig] = useState<HubPairingConfig>({ serverUrl: "", authKey: "" });
  const [qrSvg, setQrSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    invoke<HubPairingConfig | null>("get_hub_pairing_config")
      .then((saved) => saved && setConfig(saved))
      .catch((err) => setError(String(err)));
  }, []);

  async function handleGenerate() {
    setError(null);
    setBusy(true);
    setQrSvg(null);
    try {
      await invoke("save_hub_pairing_config", { serverUrl: config.serverUrl, authKey: config.authKey });
      const svg = await invoke<string>("hub_pairing_qr_svg");
      setQrSvg(svg);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Pareamento por QR</h3>
      </div>
      <p className="settings-hint">
        Preencha os dados deste hub uma vez e gere um QR — escaneie no app pra preencher a conexão automaticamente, sem
        digitar o endereço e a chave na mão.
      </p>

      <label className="settings-field">
        <span className="settings-label">Server URL</span>
        <input
          className="settings-input"
          type="text"
          placeholder="ws://192.168.x.x:7420"
          value={config.serverUrl}
          onChange={(e) => setConfig((c) => ({ ...c, serverUrl: e.currentTarget.value }))}
        />
      </label>
      <label className="settings-field">
        <span className="settings-label">Auth key</span>
        <input
          className="settings-input"
          type="password"
          value={config.authKey}
          onChange={(e) => setConfig((c) => ({ ...c, authKey: e.currentTarget.value }))}
        />
      </label>

      {error && <p className="settings-error-banner">{error}</p>}

      <button type="button" className="settings-save-btn" onClick={handleGenerate} disabled={busy}>
        Salvar e gerar QR
      </button>

      {qrSvg && (
        <div className="sync-qr-card">
          <div className="sync-qr-image" dangerouslySetInnerHTML={{ __html: qrSvg }} />
        </div>
      )}
    </section>
  );
}

function WorkspaceView() {
  const [devices, setDevices] = useState<PairedDevice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pendingActionId, setPendingActionId] = useState<string | null>(null);

  function refresh() {
    invoke<PairedDevice[]>("list_paired_devices")
      .then(setDevices)
      .catch((err) => setError(String(err)));
  }

  // Fetched fresh every time this view mounts, same as `UsageView`/`SettingsView` — pairing state
  // can change from the CLI (`warden-server devices approve/revoke`) between visits.
  useEffect(refresh, []);

  async function runAction(command: "approve_paired_device" | "revoke_paired_device", deviceId: string) {
    setActionError(null);
    setPendingActionId(deviceId);
    try {
      await invoke(command, { deviceId });
      refresh();
    } catch (err) {
      setActionError(String(err));
    } finally {
      setPendingActionId(null);
    }
  }

  if (error) {
    return (
      <div className="settings-view">
        <h2 className="settings-title">Workspace</h2>
        <p className="usage-error">{error}</p>
      </div>
    );
  }

  if (!devices) {
    return (
      <div className="settings-view">
        <p>Loading devices…</p>
      </div>
    );
  }

  return (
    <div className="settings-view">
      <h2 className="settings-title">Workspace</h2>
      <p className="settings-hint">
        Devices that have said hello to the <code>warden-server</code> hub running on <strong>this machine</strong> — a
        device paired to a hub running elsewhere won't show up here. Approving a device lets it call, or be called by,{" "}
        <code>CallDeviceTool</code> routing (Fase 9.3); it doesn't affect plain chat, which never required approval.
      </p>

      <HubPairingQrSection />

      {actionError && <p className="settings-error-banner">{actionError}</p>}

      {devices.length === 0 ? (
        <p className="settings-hint">No devices have connected to this server yet.</p>
      ) : (
        <div className="workspace-device-list">
          {devices.map((device) => (
            <div className="workspace-device-row" key={device.deviceId}>
              <div className="workspace-device-info">
                <div className="workspace-device-name-row">
                  <span className="workspace-device-name">{device.deviceName}</span>
                  <StatusBadge status={device.status} />
                </div>
                <span className="workspace-device-meta">
                  {device.deviceId} · first seen {formatSeen(device.firstSeenMs)} · last seen {formatSeen(device.lastSeenMs)}
                </span>
              </div>
              <div className="workspace-device-actions">
                {device.status === "pending" && (
                  <button
                    type="button"
                    className="settings-save-btn"
                    disabled={pendingActionId === device.deviceId}
                    onClick={() => runAction("approve_paired_device", device.deviceId)}
                  >
                    Approve
                  </button>
                )}
                {device.status === "approved" && (
                  <button
                    type="button"
                    className="provider-delete-btn"
                    disabled={pendingActionId === device.deviceId}
                    onClick={() => runAction("revoke_paired_device", device.deviceId)}
                  >
                    Revoke
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default WorkspaceView;
