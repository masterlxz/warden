import { useState } from "react";
import type { DiscoveredHub } from "../background/discovery";
import type { ConnectionSettings, DiscoverHubsResponse } from "../background/popup_protocol";
import { supportsHubDiscovery } from "../background/platform";

const DEFAULT_PORT = 7420;

interface Props {
  savedSettings: Partial<ConnectionSettings>;
  errorMessage?: string;
  busy: boolean;
  onConnect: (settings: ConnectionSettings) => void;
}

export default function ConnectionForm({ savedSettings, errorMessage, busy, onConnect }: Props) {
  const [host, setHost] = useState(savedSettings.host ?? "127.0.0.1");
  const [port, setPort] = useState(String(savedSettings.port ?? DEFAULT_PORT));
  const [deviceName, setDeviceName] = useState(savedSettings.deviceName ?? "Browser extension");
  const [authKey, setAuthKey] = useState(savedSettings.authKey ?? "");

  const [hubs, setHubs] = useState<DiscoveredHub[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | undefined>(undefined);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const parsedPort = Number.parseInt(port, 10);
    if (!host.trim() || !authKey.trim() || Number.isNaN(parsedPort)) return;
    onConnect({ host: host.trim(), port: parsedPort, deviceName: deviceName.trim() || "Browser extension", authKey: authKey.trim() });
  }

  // Fase 9.1 (redefined) — sweeps the LAN instead of asking the user to already know the IP. Only
  // ever fills host/port: the probe's reply never carries the auth key, so that stays manual.
  // Sweeps whatever port is already typed in the form (same field used for manual connect) so a
  // hub started with `--listen` on a non-default port is still discoverable.
  function handleDiscover() {
    setSearchError(undefined);
    setSearching(true);
    setHubs(null);
    const parsedPort = Number.parseInt(port, 10);
    chrome.runtime.sendMessage({ type: "discoverHubs", port: Number.isNaN(parsedPort) ? DEFAULT_PORT : parsedPort }).then((res: DiscoverHubsResponse) => {
      setSearching(false);
      if (res.ok) {
        setHubs(res.hubs);
      } else {
        setSearchError(res.error ?? "discovery failed");
      }
    });
  }

  return (
    <form className="connection-form" onSubmit={handleSubmit}>
      {/* P69 item 2 — Firefox can't learn the local subnet (no `system.network`), so no sweep there. */}
      {supportsHubDiscovery() && (
        <button type="button" onClick={handleDiscover} disabled={searching}>
          {searching ? "Procurando…" : "Procurar hubs na rede"}
        </button>
      )}
      {searchError && <p className="error-banner">{searchError}</p>}
      {hubs && hubs.length === 0 && <p className="hub-empty">Nenhum hub respondeu na rede local.</p>}
      {hubs && hubs.length > 0 && (
        <ul className="hub-list">
          {hubs.map((hub) => (
            <li key={`${hub.host}:${hub.port}`}>
              <button
                type="button"
                className="hub-item"
                onClick={() => {
                  setHost(hub.host);
                  setPort(String(hub.port));
                  setHubs(null);
                }}
              >
                <span className="hub-item-name">{hub.serverName}</span>
                <span className="hub-item-meta">
                  {hub.host}:{hub.port}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <label>
        Host
        <input value={host} onChange={(e) => setHost(e.target.value)} placeholder="127.0.0.1" required />
      </label>
      <label>
        Porta
        <input value={port} onChange={(e) => setPort(e.target.value)} inputMode="numeric" required />
      </label>
      <label>
        Nome do dispositivo
        <input value={deviceName} onChange={(e) => setDeviceName(e.target.value)} required />
      </label>
      <label>
        Auth key
        <input value={authKey} onChange={(e) => setAuthKey(e.target.value)} type="password" required />
      </label>
      {errorMessage && <p className="error-banner">{errorMessage}</p>}
      <button type="submit" disabled={busy}>
        {busy ? "Conectando…" : "Conectar"}
      </button>
    </form>
  );
}
