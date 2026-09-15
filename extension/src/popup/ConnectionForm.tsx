import { useState } from "react";
import type { ConnectionSettings } from "../background/popup_protocol";

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

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const parsedPort = Number.parseInt(port, 10);
    if (!host.trim() || !authKey.trim() || Number.isNaN(parsedPort)) return;
    onConnect({ host: host.trim(), port: parsedPort, deviceName: deviceName.trim() || "Browser extension", authKey: authKey.trim() });
  }

  return (
    <form className="connection-form" onSubmit={handleSubmit}>
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
