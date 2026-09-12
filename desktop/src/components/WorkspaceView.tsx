import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { PairedDevice } from "../types";

const dateFormatter = new Intl.DateTimeFormat("en-US", { dateStyle: "medium", timeStyle: "short" });

function formatSeen(ms: number): string {
  return dateFormatter.format(new Date(ms));
}

function StatusBadge({ status }: { status: PairedDevice["status"] }) {
  const label = status === "pending" ? "Pending" : status === "approved" ? "Approved" : "Revoked";
  return <span className={`storage-provider-badge workspace-status-badge workspace-status-badge--${status}`}>{label}</span>;
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
