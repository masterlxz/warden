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
        Liga um <code>warden-server</code> dentro deste mesmo app — outros dispositivos (mobile, extensão, navegador)
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

/** Mirrors `UserInfoDto` (P84). */
interface Person {
  id: string;
  name: string;
  role: string;
  mustChangePassword: boolean;
  /** Fatia 2 — the tools you set for them; absent/null is the safe default. */
  tools?: string[] | null;
  /** Their own agents' names. */
  agents?: string[];
}

/** Mirrors `warden_bootstrap::users::default_member_tool`: what a member has when you never chose. */
function safeByDefault(tool: string): boolean {
  return ["read_file", "write_file", "use_skill", "read_skill_file", "manage_skill", "delegate_task", "jobs", "budget", "generate_document"].includes(tool) || tool.startsWith("tavily");
}

/** Mirrors `NEVER_FOR_MEMBERS`: never theirs, so never offered. */
const NEVER_FOR_MEMBERS = ["delegate_to_agent", "message_agent", "manage_agents", "manage_tasks", "usage_stats"];

interface PeoplePayload {
  users: Person[];
  tempPassword: string | null;
}

type PeopleDraft =
  | { kind: "add"; id: string; name: string }
  | { kind: "rename"; id: string; name: string }
  /** `null`: the safe default. */
  | { kind: "tools"; id: string; tools: string[] | null };

/**
 * P84 — the members of this workspace besides you: `[[users]]` in config.toml, which syncs, so someone
 * added here can sign in on the hub on your VPS too once the file gets there. Each signs in on the web
 * or the phone with a username and password, and has their own vault and conversations. A provisional
 * password is shown once; they pick their own on first sign-in.
 */
function PeopleSection() {
  const [people, setPeople] = useState<Person[] | null>(null);
  const [draft, setDraft] = useState<PeopleDraft | null>(null);
  const [shown, setShown] = useState<{ id: string; password: string } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [toolNames, setToolNames] = useState<string[]>([]);

  useEffect(() => {
    invoke<PeoplePayload>("list_people")
      .then((p) => setPeople(p.users))
      .catch((err) => setError(String(err)));
    invoke<string[]>("list_tool_names")
      .then((names) => setToolNames(names.filter((n) => !NEVER_FOR_MEMBERS.includes(n))))
      .catch(() => setToolNames([]));
  }, []);

  async function run(command: string, args: Record<string, unknown>, shownFor?: string) {
    setError(null);
    try {
      const payload = await invoke<PeoplePayload>(command, args);
      setPeople(payload.users);
      if (payload.tempPassword && shownFor) setShown({ id: shownFor, password: payload.tempPassword });
      setDraft(null);
      setConfirmRemove(null);
    } catch (err) {
      setError(String(err));
    }
  }

  async function save() {
    if (!draft) return;
    if (draft.kind === "add") {
      const id = draft.id.trim().toLowerCase();
      await run("add_person", { id, name: draft.name }, id);
    } else if (draft.kind === "tools") {
      await run("set_person_tools", { id: draft.id, tools: draft.tools });
    } else {
      await run("rename_person", { id: draft.id, name: draft.name });
    }
  }

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">People</h3>
        <button type="button" className="settings-browse-btn" disabled={draft !== null} onClick={() => setDraft({ kind: "add", id: "", name: "" })}>
          + Add a person
        </button>
      </div>
      <p className="settings-hint">
        Others who use this Warden. Each signs in on the web or the phone with their own username and password, and has their own vault and
        conversations, which you don&apos;t see. They talk to the agents you share with them (Settings → Agents), always with their own
        memory and only the tools you allow them here, and can make agents of their own.
      </p>
      {error && <p className="settings-error-banner">{error}</p>}
      {shown && (
        <div className="provider-card">
          <p className="settings-hint">
            Provisional password for <strong>{shown.id}</strong> (shown only now): <code>{shown.password}</code>
          </p>
          <p className="settings-hint">Give it to them with the username. They choose their own the first time they sign in.</p>
          <button type="button" className="settings-browse-btn" onClick={() => setShown(null)}>
            Done
          </button>
        </div>
      )}

      {draft?.kind === "tools" && (
        <div className="provider-card skill-editor">
          <span className="settings-label">Tools for {draft.id}</span>
          <span className="settings-checkbox-row">
            <input
              type="checkbox"
              id="person-tools-default"
              checked={draft.tools === null}
              onChange={(e) => setDraft({ ...draft, tools: e.currentTarget.checked ? null : toolNames.filter(safeByDefault) })}
            />
            <label htmlFor="person-tools-default">The safe default (their own vault's files and skills, web search)</label>
          </span>
          {draft.tools !== null &&
            toolNames.map((tool) => (
              <span key={tool} className="settings-checkbox-row">
                <input
                  type="checkbox"
                  id={`person-tool-${tool}`}
                  checked={draft.tools!.includes(tool)}
                  onChange={(e) => setDraft({ ...draft, tools: e.currentTarget.checked ? [...draft.tools!, tool] : draft.tools!.filter((t) => t !== tool) })}
                />
                <label htmlFor={`person-tool-${tool}`}>
                  {tool}
                  {!safeByDefault(tool) && <span className="settings-hint"> — reaches what's yours (shell, nodes, integrations)</span>}
                </label>
              </span>
            ))}
          <span className="settings-hint">An agent never goes past this, nor past its own tools.</span>
          <div className="skill-editor-actions">
            <button type="button" className="settings-save-btn" onClick={() => void save()}>
              Save
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setDraft(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {draft && draft.kind !== "tools" && (
        <div className="provider-card skill-editor">
          {draft.kind === "add" && (
            <label className="settings-field">
              <span className="settings-label">Username</span>
              <input className="settings-input" type="text" placeholder="ana" value={draft.id} onChange={(e) => setDraft({ ...draft, id: e.currentTarget.value })} />
              <span className="settings-hint">Lowercase letters, digits, - or _. It&apos;s how they sign in.</span>
            </label>
          )}
          <label className="settings-field">
            <span className="settings-label">Name</span>
            <input className="settings-input" type="text" placeholder="Ana Souza" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.currentTarget.value })} />
          </label>
          <div className="skill-editor-actions">
            <button type="button" className="settings-save-btn" disabled={draft.name.trim() === "" || (draft.kind === "add" && draft.id.trim() === "")} onClick={() => void save()}>
              {draft.kind === "add" ? "Add" : "Save"}
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setDraft(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {people === null ? (
        <p className="settings-hint">Loading…</p>
      ) : people.length === 0 ? (
        <p className="settings-hint">Just you for now.</p>
      ) : (
        <div className="workspace-device-list">
          {people.map((person) => (
            <div className="workspace-device-row" key={person.id}>
              <div className="workspace-device-info">
                <div className="workspace-device-name-row">
                  <span className="workspace-device-name">{person.name}</span>
                  <span className={`storage-provider-badge workspace-status-badge workspace-status-badge--${person.mustChangePassword ? "pending" : "approved"}`}>
                    {person.mustChangePassword ? "provisional password" : "active"}
                  </span>
                </div>
                <span className="workspace-device-meta">
                  {person.id} · {person.tools == null ? "default tools" : person.tools.length === 0 ? "no tools" : `tools: ${person.tools.join(", ")}`}
                  {person.agents && person.agents.length > 0 ? ` · own agents: ${person.agents.join(", ")}` : ""}
                </span>
                {confirmRemove === person.id && (
                  <span className="settings-hint">
                    {person.name} leaves the workspace and their devices are disconnected. Their vault and conversations stay on disk.
                  </span>
                )}
              </div>
              <div className="workspace-device-actions">
                {confirmRemove === person.id ? (
                  <>
                    <button type="button" className="provider-delete-btn" onClick={() => void run("remove_person", { id: person.id })}>
                      Remove
                    </button>
                    <button type="button" className="settings-browse-btn" onClick={() => setConfirmRemove(null)}>
                      Cancel
                    </button>
                  </>
                ) : (
                  <>
                    <button type="button" className="settings-browse-btn" disabled={draft !== null} onClick={() => setDraft({ kind: "rename", id: person.id, name: person.name })}>
                      Rename
                    </button>
                    <button type="button" className="settings-browse-btn" disabled={draft !== null} onClick={() => setDraft({ kind: "tools", id: person.id, tools: person.tools ?? null })}>
                      Tools
                    </button>
                    <button type="button" className="settings-browse-btn" onClick={() => void run("reset_person_password", { id: person.id }, person.id)}>
                      New password
                    </button>
                    <button type="button" className="provider-delete-btn" onClick={() => setConfirmRemove(person.id)}>
                      Remove
                    </button>
                  </>
                )}
              </div>
            </div>
          ))}
        </div>
      )}

      <SharedSpacesSection people={people ?? []} />
    </section>
  );
}

/** Mirrors `SpaceDto` (P84 fatia 3). */
interface SharedSpace {
  id: string;
  folder: string;
  /** Usernames, or `"*"` for everyone. */
  readers: string[];
  /** Writers also read. */
  writers: string[];
}

const EVERYONE = "*";

type SpaceAccess = "none" | "read" | "write";

function spaceAccess(space: SharedSpace, who: string): SpaceAccess {
  if (space.writers.includes(who)) return "write";
  if (space.readers.includes(who)) return "read";
  return "none";
}

function withSpaceAccess(space: SharedSpace, who: string, access: SpaceAccess): SharedSpace {
  const readers = space.readers.filter((r) => r !== who);
  const writers = space.writers.filter((w) => w !== who);
  if (access === "read") readers.push(who);
  if (access === "write") writers.push(who);
  return { ...space, readers, writers };
}

/**
 * P84 fatia 3 — folders of your vault that members see inside theirs, at `compartilhado/<name>/`:
 * `[[spaces]]` in config.toml, synced like the people above. An agent talking to a member reads (and,
 * if you let it, writes) only in these folders, never the rest of your vault.
 */
function SharedSpacesSection({ people }: { people: Person[] }) {
  const [spaces, setSpaces] = useState<SharedSpace[] | null>(null);
  /** `originalId` absent: a new space. */
  const [draft, setDraft] = useState<{ originalId?: string; space: SharedSpace } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<SharedSpace[]>("list_shared_spaces")
      .then(setSpaces)
      .catch((err) => setError(String(err)));
  }, []);

  async function run(command: string, args: Record<string, unknown>) {
    setError(null);
    try {
      setSpaces(await invoke<SharedSpace[]>(command, args));
      setDraft(null);
      setConfirmRemove(null);
    } catch (err) {
      setError(String(err));
    }
  }

  const name = (id: string) => (id === EVERYONE ? "everyone" : (people.find((p) => p.id === id)?.name ?? id));
  const who = (space: SharedSpace) => {
    const parts = [];
    if (space.writers.length > 0) parts.push(`write: ${space.writers.map(name).join(", ")}`);
    if (space.readers.length > 0) parts.push(`read: ${space.readers.map(name).join(", ")}`);
    return parts.length > 0 ? parts.join(" · ") : "no one yet";
  };

  const accessSelect = (space: SharedSpace, person: string) => (
    <select className="settings-input" value={spaceAccess(space, person)} onChange={(e) => draft && setDraft({ ...draft, space: withSpaceAccess(space, person, e.currentTarget.value as SpaceAccess) })}>
      <option value="none">Doesn&apos;t see it</option>
      <option value="read">Reads</option>
      <option value="write">Reads and writes</option>
    </select>
  );

  return (
    <>
      <div className="settings-section-header">
        <h3 className="settings-section-title">Shared spaces</h3>
        <button
          type="button"
          className="settings-browse-btn"
          disabled={draft !== null || people.length === 0}
          onClick={() => setDraft({ space: { id: "", folder: "", readers: [], writers: [] } })}
        >
          + Share a folder
        </button>
      </div>
      <p className="settings-hint">
        Folders of your vault the people above see inside theirs, under <code>compartilhado/</code>. An agent talking to them reads (and, if you
        let it, writes) only in these folders, never the rest of your memory. Only you create spaces.
      </p>
      {error && <p className="settings-error-banner">{error}</p>}

      {draft && (
        <div className="provider-card skill-editor">
          <label className="settings-field">
            <span className="settings-label">Name</span>
            <input className="settings-input" type="text" placeholder="casa" value={draft.space.id} onChange={(e) => setDraft({ ...draft, space: { ...draft.space, id: e.currentTarget.value } })} />
            <span className="settings-hint">Lowercase letters, digits, - or _. They see it at compartilhado/{draft.space.id.trim().toLowerCase() || "name"}/.</span>
          </label>
          <label className="settings-field">
            <span className="settings-label">Folder of your vault</span>
            <input className="settings-input" type="text" placeholder="casa" value={draft.space.folder} onChange={(e) => setDraft({ ...draft, space: { ...draft.space, folder: e.currentTarget.value } })} />
            <span className="settings-hint">Relative to the vault&apos;s root, like casa or viagens/2026. Created when someone first writes in it.</span>
          </label>
          <label className="settings-field">
            <span className="settings-label">Everyone</span>
            {accessSelect(draft.space, EVERYONE)}
          </label>
          {people.map((person) => (
            <label className="settings-field" key={person.id}>
              <span className="settings-label">{person.name}</span>
              {accessSelect(draft.space, person.id)}
            </label>
          ))}
          <div className="skill-editor-actions">
            <button
              type="button"
              className="settings-save-btn"
              disabled={draft.space.id.trim() === "" || draft.space.folder.trim() === ""}
              onClick={() => void run("save_shared_space", { originalId: draft.originalId ?? null, space: { ...draft.space, id: draft.space.id.trim().toLowerCase(), folder: draft.space.folder.trim() } })}
            >
              {draft.originalId ? "Save" : "Share"}
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setDraft(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {spaces === null ? (
        <p className="settings-hint">Loading…</p>
      ) : spaces.length === 0 ? (
        <p className="settings-hint">{people.length === 0 ? "Add someone before sharing a folder." : "No folder shared."}</p>
      ) : (
        <div className="workspace-device-list">
          {spaces.map((space) => (
            <div className="workspace-device-row" key={space.id}>
              <div className="workspace-device-info">
                <div className="workspace-device-name-row">
                  <span className="workspace-device-name">{space.id}</span>
                  <code>{space.folder}</code>
                </div>
                <span className="workspace-device-meta">{who(space)}</span>
                {confirmRemove === space.id && (
                  <span className="settings-hint">Whoever saw this folder stops seeing it from their next message. The folder and its notes stay in your vault.</span>
                )}
              </div>
              <div className="workspace-device-actions">
                {confirmRemove === space.id ? (
                  <>
                    <button type="button" className="provider-delete-btn" onClick={() => void run("remove_shared_space", { id: space.id })}>
                      Stop sharing
                    </button>
                    <button type="button" className="settings-browse-btn" onClick={() => setConfirmRemove(null)}>
                      Cancel
                    </button>
                  </>
                ) : (
                  <>
                    <button type="button" className="settings-browse-btn" disabled={draft !== null} onClick={() => setDraft({ originalId: space.id, space })}>
                      Edit
                    </button>
                    <button type="button" className="provider-delete-btn" onClick={() => setConfirmRemove(space.id)}>
                      Stop sharing
                    </button>
                  </>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

/** Mirrors `LendConfigPayload` (P97). */
interface LendConfig {
  hubUrl: string;
  name: string;
  description: string;
  tags: string[];
  shell: boolean;
  files: string | null;
  mcp: string[];
  models: string[];
}

/** Mirrors `NodeState` (`warden_server::node_client`). */
type LendState =
  | { state: "connecting" }
  | { state: "connected" }
  | { state: "retrying"; error: string; inSecs: number }
  | { state: "stopped"; error: string };

interface LendActivity {
  at: number;
  kind: string;
  summary: string;
  error: string | null;
}

interface LendStatus {
  config: LendConfig | null;
  enabled: boolean;
  state: LendState | null;
  deviceId: string | null;
  paired: boolean;
  activity: LendActivity[];
}

interface LendOptions {
  mcpServers: string[];
  models: string[];
  defaultName: string;
}

const EMPTY_LEND: LendConfig = { hubUrl: "", name: "", description: "", tags: [], shell: false, files: null, mcp: [], models: [] };

function lendStateLabel(state: LendState | null): string {
  if (!state) return "Off";
  switch (state.state) {
    case "connecting":
      return "Connecting…";
    case "connected":
      return "Connected";
    case "retrying":
      return `Hub unreachable, trying again in ${state.inSecs}s: ${state.error}`;
    case "stopped":
      return `Stopped: ${state.error}`;
  }
}

function toggle(list: string[], item: string, on: boolean): string[] {
  return on ? [...list, item] : list.filter((x) => x !== item);
}

/**
 * P97 — this computer as a node of another hub (the one on your VPS): the same thing as
 * `warden-server node`, from here. You pick what it lends; the hub still has to approve it as a
 * device and allow it for its agents. Saved in hub-local.json, which doesn't sync, so it's only on
 * for this computer. The pairing key is used once and never saved.
 */
function LendSection() {
  const [status, setStatus] = useState<LendStatus | null>(null);
  const [options, setOptions] = useState<LendOptions>({ mcpServers: [], models: [], defaultName: "" });
  const [form, setForm] = useState<LendConfig>(EMPTY_LEND);
  const [tagsText, setTagsText] = useState("");
  const [authKey, setAuthKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  function apply(next: LendStatus) {
    setStatus(next);
    if (next.config) {
      setForm(next.config);
      setTagsText(next.config.tags.join(", "));
    }
  }

  useEffect(() => {
    invoke<LendStatus>("get_lend_status")
      .then(apply)
      .catch((err) => setError(String(err)));
    invoke<LendOptions>("lend_options")
      .then(setOptions)
      .catch(() => {});
  }, []);

  const running = status?.state != null;

  // While on: the state and the activity move on their own.
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => {
      invoke<LendStatus>("get_lend_status")
        .then(setStatus)
        .catch(() => {});
    }, 3000);
    return () => window.clearInterval(timer);
  }, [running]);

  async function start() {
    setError(null);
    setBusy(true);
    try {
      const config = { ...form, tags: tagsText.split(",").map((t) => t.trim()).filter(Boolean) };
      apply(await invoke<LendStatus>("start_lending", { config, authKey: authKey.trim() || null }));
      setAuthKey("");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    setError(null);
    try {
      apply(await invoke<LendStatus>("stop_lending"));
    } catch (err) {
      setError(String(err));
    }
  }

  async function pickFolder() {
    const selected = await open({ multiple: false, directory: true });
    if (typeof selected === "string") setForm((f) => ({ ...f, files: selected }));
  }

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Lend this computer</h3>
        {running ? (
          <button type="button" className="provider-delete-btn" onClick={() => void stop()}>
            Stop lending
          </button>
        ) : (
          <button type="button" className="settings-save-btn" disabled={busy || form.hubUrl.trim() === ""} onClick={() => void start()}>
            {busy ? "Starting…" : "Start lending"}
          </button>
        )}
      </div>
      <p className="settings-hint">
        Lets the agents of another hub (like the one on your VPS) use this computer. On that hub, approve it in the device list and allow it under Nodes.
      </p>
      {error && <p className="settings-error-banner">{error}</p>}
      {status && (
        <p className="settings-hint">
          <strong>{lendStateLabel(status.state)}</strong>
          {status.deviceId ? ` · device id ${status.deviceId}` : ""}
        </p>
      )}

      <div className="provider-card skill-editor">
        <label className="settings-field">
          <span className="settings-label">Hub address</span>
          <input
            className="settings-input"
            type="text"
            placeholder="wss://my-vps.tailnet.ts.net:7420"
            value={form.hubUrl}
            disabled={running}
            onChange={(e) => setForm({ ...form, hubUrl: e.currentTarget.value })}
          />
        </label>
        {!running && (
          <>
            <ApiKeyField label={status?.paired ? "Hub pairing key (only to pair again)" : "Hub pairing key (only the first time)"} value={authKey} onChange={setAuthKey} />
            <span className="settings-hint">Used once to join the hub, never saved: the hub gives this computer its own token.</span>
          </>
        )}
        <label className="settings-field">
          <span className="settings-label">Name on the hub</span>
          <input
            className="settings-input"
            type="text"
            placeholder={options.defaultName}
            value={form.name}
            disabled={running}
            onChange={(e) => setForm({ ...form, name: e.currentTarget.value })}
          />
        </label>
        <label className="settings-field">
          <span className="settings-label">Description for the agents</span>
          <input
            className="settings-input"
            type="text"
            placeholder="Home PC with the GPU and the photo archive"
            value={form.description}
            disabled={running}
            onChange={(e) => setForm({ ...form, description: e.currentTarget.value })}
          />
        </label>
        <label className="settings-field">
          <span className="settings-label">Tags</span>
          <input className="settings-input" type="text" placeholder="home, gpu" value={tagsText} disabled={running} onChange={(e) => setTagsText(e.currentTarget.value)} />
        </label>
        <label className="settings-field settings-checkbox-field">
          <span className="settings-checkbox-row">
            <input type="checkbox" checked={form.shell} disabled={running} onChange={(e) => setForm({ ...form, shell: e.currentTarget.checked })} />
            <span className="settings-label">Its shell</span>
          </span>
          <span className="settings-hint">Commands run in the shared folder, or your home folder if none.</span>
        </label>
        <div className="settings-field">
          <span className="settings-label">A folder</span>
          <div className="settings-checkbox-row">
            <input className="settings-input" type="text" readOnly placeholder="None" value={form.files ?? ""} />
            <button type="button" className="settings-browse-btn" disabled={running} onClick={() => void pickFolder()}>
              Choose…
            </button>
            {form.files && (
              <button type="button" className="settings-browse-btn" disabled={running} onClick={() => setForm({ ...form, files: null })}>
                Clear
              </button>
            )}
          </div>
        </div>
        {options.mcpServers.length > 0 && (
          <div className="settings-field">
            <span className="settings-label">MCP servers</span>
            <div className="skill-agent-list">
              {options.mcpServers.map((name) => (
                <label className="skill-agent-option" key={name}>
                  <input type="checkbox" checked={form.mcp.includes(name)} disabled={running} onChange={(e) => setForm({ ...form, mcp: toggle(form.mcp, name, e.currentTarget.checked) })} />
                  {name}
                </label>
              ))}
            </div>
          </div>
        )}
        {options.models.length > 0 && (
          <div className="settings-field">
            <span className="settings-label">Models</span>
            <div className="skill-agent-list">
              {options.models.map((id) => (
                <label className="skill-agent-option" key={id}>
                  <input type="checkbox" checked={form.models.includes(id)} disabled={running} onChange={(e) => setForm({ ...form, models: toggle(form.models, id, e.currentTarget.checked) })} />
                  {id}
                </label>
              ))}
            </div>
            <span className="settings-hint">The hub uses a lent model through a provider of kind "A node's model".</span>
          </div>
        )}
      </div>

      {running && (
        <>
          <h4 className="settings-label">What the agents did here</h4>
          {status.activity.length === 0 ? (
            <p className="settings-hint">Nothing yet.</p>
          ) : (
            <div className="workspace-device-list">
              {status.activity.map((entry, i) => (
                <div className="workspace-device-row" key={`${entry.at}-${i}`}>
                  <div className="workspace-device-info">
                    <span className="workspace-device-name">
                      {entry.kind} {entry.summary && <code>{entry.summary}</code>}
                    </span>
                    <span className="workspace-device-meta">
                      {formatSeen(entry.at * 1000)}
                      {entry.error ? ` · failed: ${entry.error}` : ""}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </section>
  );
}

/** Mirrors `NodeInfoDto` (P93). */
interface NodeInfo {
  deviceId: string;
  name: string;
  online: boolean;
  approved: boolean;
  offer?: { description: string; tags: string[]; shell: boolean; files: boolean; mcpTools?: { name: string }[]; models?: string[] };
  enabled: boolean;
  agents: string[];
  requireApproval: boolean;
}

interface NodeDraft {
  deviceId: string;
  enabled: boolean;
  agents: string[];
  requireApproval: boolean;
}

function offerLabel(node: NodeInfo): string {
  if (!node.offer) return "not connected to this machine's hub since it started";
  const mcp = node.offer.mcpTools ?? [];
  const parts = [
    node.offer.shell && "its shell",
    node.offer.files && "a folder",
    mcp.length > 0 && `${mcp.length} MCP tool(s) (${mcp.map((t) => t.name).join(", ")})`,
    (node.offer.models ?? []).length > 0 && `the model(s) ${(node.offer.models ?? []).join(", ")}`,
  ].filter(Boolean);
  return parts.length ? `lends ${parts.join(" and ")}` : "lends nothing";
}

/**
 * P93 — machines running `warden-server node` that lend their shell or a folder to your agents. Two
 * locks: the node's operator chose what it offers; here you pick whether agents may use it, which
 * ones, and whether each call asks you first. It must also be approved in the device list below. The
 * entries are saved in config.toml, which syncs — so a node that joins the hub on your VPS can be
 * allowed from here by typing its id.
 */
function NodesSection({ refreshKey }: { refreshKey: number }) {
  const [nodes, setNodes] = useState<NodeInfo[] | null>(null);
  const [agentIds, setAgentIds] = useState<string[]>([]);
  const [draft, setDraft] = useState<NodeDraft | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<NodeInfo[]>("list_nodes")
      .then(setNodes)
      .catch((err) => setError(String(err)));
    invoke<{ agents: { id: string }[] }>("get_settings")
      .then((settings) => setAgentIds(settings.agents.map((a) => a.id)))
      .catch(() => setAgentIds([]));
  }, [refreshKey]);

  async function save() {
    if (!draft) return;
    setError(null);
    try {
      setNodes(await invoke<NodeInfo[]>("save_node_access", { ...draft }));
      setDraft(null);
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Nodes</h3>
        <button type="button" className="settings-browse-btn" onClick={() => setDraft({ deviceId: "", enabled: true, agents: [], requireApproval: false })}>
          + Allow a node by id
        </button>
      </div>
      <p className="settings-hint">
        Other machines lending their shell or a folder to your agents: <code>warden-server node --hub … --shell --files &lt;folder&gt;</code>.
      </p>
      {error && <p className="settings-error-banner">{error}</p>}

      {draft && (
        <div className="provider-card skill-editor">
          <label className="settings-field">
            <span className="settings-label">Node device id</span>
            <input
              className="settings-input"
              type="text"
              placeholder="node-home-pc-1a2b3c4d"
              value={draft.deviceId}
              disabled={nodes?.some((n) => n.deviceId === draft.deviceId) && draft.deviceId !== ""}
              onChange={(e) => setDraft({ ...draft, deviceId: e.currentTarget.value })}
            />
          </label>
          <label className="settings-field settings-checkbox-field">
            <span className="settings-checkbox-row">
              <input type="checkbox" checked={draft.enabled} onChange={(e) => setDraft({ ...draft, enabled: e.currentTarget.checked })} />
              <span className="settings-label">Agents may use it</span>
            </span>
          </label>
          <div className="settings-field">
            <span className="settings-label">Only these agents</span>
            <div className="skill-agent-list">
              {agentIds.map((id) => (
                <label className="skill-agent-option" key={id}>
                  <input
                    type="checkbox"
                    checked={draft.agents.includes(id)}
                    onChange={(e) =>
                      setDraft({ ...draft, agents: e.currentTarget.checked ? [...draft.agents, id] : draft.agents.filter((a) => a !== id) })
                    }
                  />
                  {id}
                </label>
              ))}
            </div>
            <span className="settings-hint">None ticked means every agent.</span>
          </div>
          <label className="settings-field settings-checkbox-field">
            <span className="settings-checkbox-row">
              <input type="checkbox" checked={draft.requireApproval} onChange={(e) => setDraft({ ...draft, requireApproval: e.currentTarget.checked })} />
              <span className="settings-label">Ask me before every command or file</span>
            </span>
            <span className="settings-hint">Scheduled tasks can't use a node that asks, since nobody is there to answer.</span>
          </label>
          <div className="skill-editor-actions">
            <button type="button" className="settings-save-btn" disabled={draft.deviceId.trim() === ""} onClick={() => void save()}>
              Save
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setDraft(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {nodes === null ? (
        <p className="settings-hint">Loading nodes…</p>
      ) : nodes.length === 0 ? (
        <p className="settings-hint">No nodes yet.</p>
      ) : (
        <div className="workspace-device-list">
          {nodes.map((node) => (
            <div className="workspace-device-row" key={node.deviceId}>
              <div className="workspace-device-info">
                <div className="workspace-device-name-row">
                  <span className="workspace-device-name">
                    {node.name} {node.online ? "· online" : ""}
                  </span>
                  <span className={`storage-provider-badge workspace-status-badge workspace-status-badge--${node.enabled && node.approved ? "approved" : "pending"}`}>
                    {!node.approved ? "needs approval" : node.enabled ? "allowed" : "blocked"}
                  </span>
                </div>
                <span className="workspace-device-meta">
                  {node.deviceId} · {offerLabel(node)}
                  {node.offer?.description ? ` · ${node.offer.description}` : ""}
                  {node.offer && node.offer.tags.length > 0 ? ` · ${node.offer.tags.join(", ")}` : ""}
                  {" · "}
                  {node.agents.length === 0 ? "every agent" : `only ${node.agents.join(", ")}`}
                  {node.requireApproval ? " · asks first" : ""}
                </span>
              </div>
              <div className="workspace-device-actions">
                <button
                  type="button"
                  className="settings-browse-btn"
                  onClick={() => setDraft({ deviceId: node.deviceId, enabled: node.enabled, agents: node.agents, requireApproval: node.requireApproval })}
                >
                  Access
                </button>
              </div>
            </div>
          ))}
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
  /** Bumped with the device list, so the nodes reread their "approved" state. */
  const [refreshKey, setRefreshKey] = useState(0);

  function refresh() {
    invoke<PairedDevice[]>("list_paired_devices")
      .then((next) => {
        setDevices(next);
        setRefreshKey((k) => k + 1);
      })
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

      <PeopleSection />
      <LendSection />
      <NodesSection refreshKey={refreshKey} />
    </div>
  );
}

export default WorkspaceView;
