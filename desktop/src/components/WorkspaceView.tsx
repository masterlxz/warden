import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ApiKeyField } from "./SettingsView";
import type { DiscoveredHub, EmbeddedServerConfig, EmbeddedServerStatus, HubPairingConfig, PairedDevice } from "../types";

const STOPPED_STATUS: EmbeddedServerStatus = { running: false, boundAddr: null, serverName: null, secure: false, secureUrl: null, webUrl: null };

const dateFormatter = new Intl.DateTimeFormat("en-US", { dateStyle: "medium", timeStyle: "short" });

function formatSeen(ms: number): string {
  return dateFormatter.format(new Date(ms));
}

function StatusBadge({ status }: { status: PairedDevice["status"] }) {
  const label = status === "pending" ? "Pending" : status === "approved" ? "Approved" : "Revoked";
  return <span className={`storage-provider-badge workspace-status-badge workspace-status-badge--${status}`}>{label}</span>;
}

type HttpsMode = "none" | "tailscale" | "own";

function newConfig(authKey: string): EmbeddedServerConfig {
  return { port: 7420, listenHost: null, authKey, serverName: null, tailscaleCert: false, tlsCert: null, tlsKey: null, tlsHost: null, webUi: true };
}

/** Fase 9.1 follow-up ("virar o hub desta rede") — lets this same desktop app embed its own
 * `warden-server` instead of that always being a separate process. Every `warden-server serve`
 * flag has a field here (Sessão 103), so nothing needs the terminal. The inputs lock while
 * running, since they only apply on the next start. */
function EmbeddedServerSection() {
  const [config, setConfig] = useState<EmbeddedServerConfig>(newConfig(""));
  const [serverNameInput, setServerNameInput] = useState("");
  const [httpsMode, setHttpsMode] = useState<HttpsMode>("none");
  const [status, setStatus] = useState<EmbeddedServerStatus>(STOPPED_STATUS);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    invoke<EmbeddedServerConfig | null>("get_embedded_server_config")
      .then(async (saved) => {
        if (saved) {
          setConfig(saved);
          setServerNameInput(saved.serverName ?? "");
          setHttpsMode(saved.tailscaleCert ? "tailscale" : saved.tlsCert ? "own" : "none");
        } else {
          const authKey = await invoke<string>("generate_embedded_server_auth_key");
          setConfig(newConfig(authKey));
        }
      })
      .catch((err) => setError(String(err)));
    invoke<EmbeddedServerStatus>("embedded_server_status")
      .then(setStatus)
      .catch(() => {});
  }, []);

  async function handleGenerateKey() {
    const authKey = await invoke<string>("generate_embedded_server_auth_key");
    setConfig((c) => ({ ...c, authKey }));
  }

  async function handleBrowse(field: "tlsCert" | "tlsKey") {
    const selected = await open({ multiple: false, directory: false });
    if (typeof selected === "string") setConfig((c) => ({ ...c, [field]: selected }));
  }

  async function handleStart() {
    setError(null);
    setBusy(true);
    try {
      // Always save first — `start_embedded_server` reads the config straight from disk, so a
      // field edited here but never saved would otherwise start the server with stale settings.
      const ownCert = httpsMode === "own";
      await invoke("save_embedded_server_config", {
        config: {
          ...config,
          serverName: serverNameInput.trim() || null,
          tailscaleCert: httpsMode === "tailscale",
          tlsCert: ownCert ? config.tlsCert : null,
          tlsKey: ownCert ? config.tlsKey : null,
          tlsHost: ownCert ? config.tlsHost : null,
        },
      });
      const next = await invoke<EmbeddedServerStatus>("start_embedded_server");
      setStatus(next);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleStop() {
    setError(null);
    setBusy(true);
    try {
      await invoke("stop_embedded_server");
      setStatus(STOPPED_STATUS);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Ser o hub desta rede</h3>
      </div>
      <p className="settings-hint">
        Liga um <code>warden-server</code> dentro deste mesmo app — outros dispositivos (mobile, extensão, outro <code>warden-node</code>)
        conseguem se conectar por aqui, na porta escolhida abaixo. Uma vez ligado, volta a subir sozinho toda vez que este app abrir; pra
        acessar de fora da rede local (tipo Jellyfin), redirecione essa porta no seu roteador.
      </p>

      <p className="settings-hint">
        {status.running ? (
          <>
            Rodando como <strong>{status.serverName}</strong> em <code>{status.boundAddr}</code>
            {status.secureUrl ? (
              <>
                {" "}
                — só HTTPS, conecte em <code>{status.secureUrl}</code>
              </>
            ) : status.secure ? (
              " — só HTTPS, pelo nome do certificado"
            ) : (
              " — sem criptografia (ws://)"
            )}
            {status.webUrl && (
              <>
                <br />
                Interface web:{" "}
                <a
                  href={status.webUrl}
                  onClick={(e) => {
                    e.preventDefault();
                    void openUrl(status.webUrl!);
                  }}
                >
                  {status.webUrl}
                </a>
                {!status.secureUrl && " (de outro aparelho da rede, troque localhost pelo IP desta máquina)"}
              </>
            )}
          </>
        ) : (
          "Parado."
        )}
      </p>

      <label className="settings-field">
        <span className="settings-label">Porta</span>
        <input
          className="settings-input"
          type="number"
          value={config.port}
          disabled={status.running}
          onChange={(e) => setConfig((c) => ({ ...c, port: Number(e.currentTarget.value) }))}
        />
      </label>
      <label className="settings-field">
        <span className="settings-label">Nome (opcional)</span>
        <input
          className="settings-input"
          type="text"
          placeholder="ex.: Desktop da sala"
          value={serverNameInput}
          disabled={status.running}
          onChange={(e) => setServerNameInput(e.currentTarget.value)}
        />
      </label>
      <label className="settings-field">
        <span className="settings-label">Endereço (opcional)</span>
        <span className="settings-hint">
          Em branco, atende em todas as interfaces de rede (<code>0.0.0.0</code>). <code>127.0.0.1</code> deixa o hub acessível só
          deste computador; o IP de uma interface (ex.: o do Tailscale) limita a ela.
        </span>
        <input
          className="settings-input"
          type="text"
          placeholder="0.0.0.0"
          value={config.listenHost ?? ""}
          disabled={status.running}
          onChange={(e) => setConfig((c) => ({ ...c, listenHost: e.currentTarget.value || null }))}
        />
      </label>
      <label className="settings-field">
        <span className="settings-label">HTTPS</span>
        <select
          className="settings-select"
          value={httpsMode}
          disabled={status.running}
          onChange={(e) => setHttpsMode(e.currentTarget.value as HttpsMode)}
        >
          <option value="none">Sem criptografia (ws://)</option>
          <option value="tailscale">Via Tailscale</option>
          <option value="own">Certificado próprio</option>
        </select>
        {httpsMode === "tailscale" && (
          <span className="settings-hint">
            Criptografa a conexão com o certificado do Tailscale deste computador (<code>tailscale cert</code>), renovado sozinho. Os
            dispositivos passam a conectar pelo nome <code>*.ts.net</code>, só de dentro da tailnet. Precisa de MagicDNS e certificados
            HTTPS ligados no painel do Tailscale e, sem root, <code>sudo tailscale set --operator=$USER</code>.
          </span>
        )}
        {httpsMode === "own" && (
          <span className="settings-hint">
            Um certificado em PEM (a cadeia, começando pelo do site) e a chave privada dele, como os do Let's Encrypt. Os arquivos
            são relidos quando mudam, então renovar não pede reiniciar.
          </span>
        )}
      </label>
      {httpsMode === "own" && (
        <>
          {(["tlsCert", "tlsKey"] as const).map((field) => (
            <label className="settings-field" key={field}>
              <span className="settings-label">{field === "tlsCert" ? "Certificado (.pem)" : "Chave privada (.pem)"}</span>
              <div className="settings-key-field">
                <input
                  className="settings-input"
                  type="text"
                  value={config[field] ?? ""}
                  disabled={status.running}
                  onChange={(e) => setConfig((c) => ({ ...c, [field]: e.currentTarget.value || null }))}
                />
                {!status.running && (
                  <button type="button" className="settings-browse-btn" onClick={() => void handleBrowse(field)}>
                    Escolher…
                  </button>
                )}
              </div>
            </label>
          ))}
          <label className="settings-field">
            <span className="settings-label">Nome do certificado (opcional)</span>
            <span className="settings-hint">
              O nome para o qual o certificado vale (ex.: <code>hub.meudominio.com</code>). Com ele, a descoberta na rede e o link da
              interface web já apontam para o endereço certo.
            </span>
            <input
              className="settings-input"
              type="text"
              placeholder="hub.meudominio.com"
              value={config.tlsHost ?? ""}
              disabled={status.running}
              onChange={(e) => setConfig((c) => ({ ...c, tlsHost: e.currentTarget.value || null }))}
            />
          </label>
        </>
      )}
      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input
            type="checkbox"
            checked={config.webUi}
            disabled={status.running}
            onChange={(e) => setConfig((c) => ({ ...c, webUi: e.currentTarget.checked }))}
          />
          <span className="settings-label">Interface web</span>
        </span>
        <span className="settings-hint">
          Abrir o endereço do hub num navegador mostra o Warden completo, que pareia como mais um dispositivo. Desligada, a porta
          atende só os apps.
        </span>
      </label>
      <ApiKeyField label="Auth key" value={config.authKey} onChange={(authKey) => setConfig((c) => ({ ...c, authKey }))} />
      {!status.running && (
        <button type="button" className="settings-browse-btn" onClick={handleGenerateKey}>
          Gerar nova chave
        </button>
      )}

      {error && <p className="settings-error-banner">{error}</p>}

      <button type="button" className="settings-save-btn" onClick={status.running ? handleStop : handleStart} disabled={busy}>
        {busy ? "Aguarde…" : status.running ? "Desligar" : "Ligar"}
      </button>
    </section>
  );
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

  const [hubs, setHubs] = useState<DiscoveredHub[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [discoveryPort, setDiscoveryPort] = useState("7420");
  // P36 — the embedded hub's own wss:// URL, offered as a one-click Server URL when it's on.
  const [embeddedSecureUrl, setEmbeddedSecureUrl] = useState<string | null>(null);

  useEffect(() => {
    invoke<HubPairingConfig | null>("get_hub_pairing_config")
      .then((saved) => saved && setConfig(saved))
      .catch((err) => setError(String(err)));
    invoke<EmbeddedServerStatus>("embedded_server_status")
      .then((status) => setEmbeddedSecureUrl(status.secureUrl))
      .catch(() => {});
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

  // Fase 9.1 (redefined) — sweeps the LAN instead of asking the operator to already know the IP.
  // Only ever fills Server URL: the auth key is never part of the probe's reply, so it stays
  // manual on purpose.
  async function handleDiscover() {
    setSearchError(null);
    setSearching(true);
    setHubs(null);
    try {
      const port = Number(discoveryPort);
      const found = await invoke<DiscoveredHub[]>("discover_hubs", { port });
      setHubs(found);
    } catch (err) {
      setSearchError(String(err));
    } finally {
      setSearching(false);
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
        <span className="settings-label">Porta a procurar</span>
        <input
          className="settings-input"
          type="text"
          inputMode="numeric"
          placeholder="7420"
          value={discoveryPort}
          onChange={(e) => setDiscoveryPort(e.currentTarget.value)}
        />
      </label>
      <button type="button" className="settings-save-btn" onClick={handleDiscover} disabled={searching}>
        {searching ? "Procurando…" : "Procurar hubs na rede"}
      </button>
      {searchError && <p className="settings-error-banner">{searchError}</p>}
      {hubs && hubs.length === 0 && <p className="settings-hint">Nenhum hub respondeu na rede local.</p>}
      {hubs && hubs.length > 0 && (
        <div className="workspace-device-list">
          {hubs.map((hub) => (
            <button
              type="button"
              key={`${hub.host}:${hub.port}`}
              className="workspace-device-row workspace-device-row--clickable"
              onClick={() => setConfig((c) => ({ ...c, serverUrl: hub.secureUrl ?? `ws://${hub.host}:${hub.port}` }))}
            >
              <div className="workspace-device-info">
                <span className="workspace-device-name">{hub.serverName}</span>
                <span className="workspace-device-meta">{hub.secureUrl ?? `${hub.host}:${hub.port}`}</span>
              </div>
            </button>
          ))}
        </div>
      )}

      <label className="settings-field">
        <span className="settings-label">Server URL</span>
        <input
          className="settings-input"
          type="text"
          placeholder="wss://hub.tailXXXX.ts.net:7420 ou ws://192.168.x.x:7420"
          value={config.serverUrl}
          onChange={(e) => setConfig((c) => ({ ...c, serverUrl: e.currentTarget.value }))}
        />
      </label>
      {embeddedSecureUrl && config.serverUrl !== embeddedSecureUrl && (
        <button type="button" className="settings-browse-btn" onClick={() => setConfig((c) => ({ ...c, serverUrl: embeddedSecureUrl }))}>
          Usar o hub deste app ({embeddedSecureUrl})
        </button>
      )}
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

      <EmbeddedServerSection />

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
