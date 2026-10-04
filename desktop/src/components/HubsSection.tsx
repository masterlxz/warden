import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DiscoveredHub, EmbeddedServerStatus, SavedHub } from "../types";

/** What the hub's address is typed as from a LAN discovery result: the web interface is on the same port. */
function webAddressOf(hub: DiscoveredHub): string {
  return hub.secureUrl ? hub.secureUrl.replace(/^wss:/, "https:") : `http://${hub.host}:${hub.port}`;
}

interface Draft {
  /** Absent: a new hub. */
  id?: string;
  name: string;
  url: string;
}

/**
 * P102 — this computer as a client of a hub, beside being one (the section above) and a node of one ("Lend
 * this computer"). Each saved hub opens in a window of its own on the web interface the hub serves, so
 * everything the hub has is there. The window gets no access to this app: the hub's page signs in like
 * any browser, and keeps its own identity per hub. Only a name and an address are saved here.
 */
function HubsSection() {
  const [hubs, setHubs] = useState<SavedHub[] | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [found, setFound] = useState<DiscoveredHub[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [discoveryPort, setDiscoveryPort] = useState("7420");
  // The web interface of the hub embedded in this app, when it is running with one.
  const [ownWebUrl, setOwnWebUrl] = useState<string | null>(null);

  const load = useCallback(() => {
    invoke<SavedHub[]>("list_hubs")
      .then(setHubs)
      .catch((err) => setError(String(err)));
    invoke<EmbeddedServerStatus>("embedded_server_status")
      .then((status) => setOwnWebUrl(status.running ? status.webUrl : null))
      .catch(() => setOwnWebUrl(null));
  }, []);

  useEffect(() => {
    load();
    // A hub window closed on its own: the "Open" mark follows when this screen is looked at again.
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, [load]);

  async function run(action: () => Promise<unknown>) {
    setError(null);
    setBusy(true);
    try {
      await action();
      load();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  const open = (id: string) => run(() => invoke("open_hub_window", { id }));

  const save = () =>
    draft &&
    run(async () => {
      await invoke("save_hub", { id: draft.id ?? null, name: draft.name, url: draft.url });
      setDraft(null);
    });

  const remove = (hub: SavedHub) => run(() => invoke("remove_hub", { id: hub.id }));

  const openOwn = () =>
    ownWebUrl &&
    run(async () => {
      const hub = await invoke<SavedHub>("ensure_hub", { name: "This computer", url: ownWebUrl });
      await invoke("open_hub_window", { id: hub.id });
    });

  async function discover() {
    setError(null);
    setSearching(true);
    setFound(null);
    try {
      setFound(await invoke<DiscoveredHub[]>("discover_hubs", { port: Number(discoveryPort) }));
    } catch (err) {
      setError(String(err));
    } finally {
      setSearching(false);
    }
  }

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Hubs</h3>
      </div>
      <p className="settings-hint">
        Use a hub that runs on another machine (your VPS, a mini-PC) from this app. Each hub opens in its own window, on
        the web interface the hub serves, so everything it has is there. That window has no access to this app, and you
        sign in on its page like in a browser. Only a name and an address are saved here, never a key or a password.
      </p>

      {hubs && hubs.length === 0 && !draft && <p className="settings-hint">No hub saved yet.</p>}
      {hubs && hubs.length > 0 && (
        <div className="workspace-device-list">
          {hubs.map((hub) => (
            <div key={hub.id} className="workspace-device-row">
              <div className="workspace-device-info">
                <span className="workspace-device-name">
                  {hub.name}
                  {hub.open && <span className="storage-provider-badge workspace-status-badge workspace-status-badge--approved"> Open</span>}
                </span>
                <span className="workspace-device-meta">{hub.url}</span>
              </div>
              <div className="workspace-device-actions">
                <button type="button" className="settings-save-btn" disabled={busy} onClick={() => open(hub.id)}>
                  {hub.open ? "Show" : "Open"}
                </button>
                <button type="button" className="settings-browse-btn" disabled={busy} onClick={() => setDraft({ id: hub.id, name: hub.name, url: hub.url })}>
                  Edit
                </button>
                <button type="button" className="provider-delete-btn" disabled={busy} onClick={() => remove(hub)}>
                  Remove
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {ownWebUrl && (
        <button type="button" className="settings-browse-btn" disabled={busy} onClick={openOwn}>
          Open this computer's own hub in a window
        </button>
      )}

      {error && <p className="settings-error-banner">{error}</p>}

      {draft ? (
        <>
          <label className="settings-field">
            <span className="settings-label">Name</span>
            <input className="settings-input" type="text" placeholder="VPS" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.currentTarget.value })} />
          </label>
          <label className="settings-field">
            <span className="settings-label">Address</span>
            <input
              className="settings-input"
              type="text"
              placeholder="https://hub.tailXXXX.ts.net or 192.168.x.x:7420"
              value={draft.url}
              onChange={(e) => setDraft({ ...draft, url: e.currentTarget.value })}
            />
          </label>
          <div className="workspace-device-actions">
            <button type="button" className="settings-save-btn" disabled={busy} onClick={save}>
              {draft.id ? "Save" : "Add hub"}
            </button>
            <button type="button" className="settings-browse-btn" disabled={busy} onClick={() => setDraft(null)}>
              Cancel
            </button>
          </div>

          <label className="settings-field">
            <span className="settings-label">Port to search</span>
            <input className="settings-input" type="text" inputMode="numeric" placeholder="7420" value={discoveryPort} onChange={(e) => setDiscoveryPort(e.currentTarget.value)} />
          </label>
          <button type="button" className="settings-browse-btn" disabled={searching} onClick={discover}>
            {searching ? "Searching…" : "Search for hubs on the network"}
          </button>
          {found && found.length === 0 && <p className="settings-hint">No hub answered on the local network.</p>}
          {found && found.length > 0 && (
            <div className="workspace-device-list">
              {found.map((hub) => (
                <button
                  type="button"
                  key={`${hub.host}:${hub.port}`}
                  className="workspace-device-row workspace-device-row--clickable"
                  onClick={() => setDraft({ ...draft, name: draft.name || hub.serverName, url: webAddressOf(hub) })}
                >
                  <div className="workspace-device-info">
                    <span className="workspace-device-name">{hub.serverName}</span>
                    <span className="workspace-device-meta">{webAddressOf(hub)}</span>
                  </div>
                </button>
              ))}
            </div>
          )}
        </>
      ) : (
        <button type="button" className="settings-save-btn" disabled={busy} onClick={() => setDraft({ name: "", url: "" })}>
          Add a hub
        </button>
      )}
    </section>
  );
}

export default HubsSection;
