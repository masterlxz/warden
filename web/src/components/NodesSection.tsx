import { useCallback, useEffect, useState } from "react";
import { NodeError, type ServerConnection } from "../hub/connection";
import type { NodeInfo } from "../hub/messages";

// Nodes (P93): machines running `warden-server node` that lend their shell and/or a folder to this
// hub's agents. Two locks: the node's operator chose what it offers; here you decide whether agents
// may use it, which ones, and whether every call asks you first. It also has to be approved in the
// device list above. Every change asks for the pairing key, like approving a device.

interface Draft {
  enabled: boolean;
  /** Empty = every agent. */
  agents: string[];
  requireApproval: boolean;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function offers(node: NodeInfo): string {
  if (!node.offer) return "ainda não se conectou desde que o hub subiu";
  const mcp = node.offer.mcpTools ?? [];
  const parts = [
    node.offer.shell && "terminal",
    node.offer.files && "arquivos de uma pasta",
    mcp.length > 0 && `${mcp.length} tool(s) MCP (${mcp.map((t) => t.name).join(", ")})`,
  ].filter(Boolean);
  return parts.length ? `empresta ${parts.join(" e ")}` : "não empresta nada";
}

export default function NodesSection({ conn, refreshKey }: { conn: ServerConnection | null; refreshKey: number }) {
  const [nodes, setNodes] = useState<NodeInfo[] | null>(null);
  const [agentIds, setAgentIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<{ deviceId: string; draft: Draft } | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!conn) return;
    try {
      setNodes(await conn.listNodes());
      setError(null);
    } catch (err) {
      setError(message(err));
    }
  }, [conn]);

  useEffect(() => {
    void load();
    conn
      ?.requestSettings()
      .then(({ settings }) => setAgentIds(settings.agents.map((a) => a.id)))
      .catch(() => setAgentIds([]));
  }, [conn, load, refreshKey]);

  function cancel() {
    setEditing(null);
    setPairingKey("");
    setKeyError(null);
  }

  async function save() {
    if (!conn || !editing) return;
    setBusy(true);
    setKeyError(null);
    try {
      const { enabled, agents, requireApproval } = editing.draft;
      setNodes(await conn.setNodeAccess(pairingKey, editing.deviceId, enabled, agents, requireApproval));
      cancel();
    } catch (err) {
      if (err instanceof NodeError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        setError(message(err));
        cancel();
      }
    } finally {
      setBusy(false);
    }
  }

  function patch(change: Partial<Draft>) {
    setEditing((current) => (current ? { ...current, draft: { ...current.draft, ...change } } : current));
  }

  return (
    <div className="nodes-section">
      <h3 className="settings-section-title">Nós</h3>
      <p className="skills-hint">
        Máquinas que emprestam o terminal ou uma pasta para os seus agentes (<code>warden-server node --hub … --shell --files &lt;pasta&gt;</code>).
        Um agente só usa um nó aprovado acima e liberado aqui.
      </p>
      {error && <p className="error-banner">{error}</p>}
      {nodes === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : nodes.length === 0 ? (
        <p className="skills-hint">Nenhum nó ainda.</p>
      ) : (
        <ul className="skills-list">
          {nodes.map((node) => {
            const isEditing = editing?.deviceId === node.deviceId;
            return (
              <li key={node.deviceId} className="skills-item">
                <div className="skills-item-header">
                  <span className="skills-item-name">
                    {node.name} {node.online ? "· online" : "· offline"}
                  </span>
                  <span className={`devices-status devices-status--${node.enabled && node.approved ? "approved" : "pending"}`}>
                    {!node.approved ? "Falta aprovar" : node.enabled ? "Liberado" : "Bloqueado"}
                  </span>
                </div>
                <p className="skills-item-description">
                  <code>{node.deviceId}</code> · {offers(node)}
                  {node.offer?.description && ` · ${node.offer.description}`}
                  {node.offer && node.offer.tags.length > 0 && ` · ${node.offer.tags.join(", ")}`}
                </p>
                <p className="skills-hint">
                  {node.agents.length === 0 ? "Todos os agentes" : `Só: ${node.agents.join(", ")}`}
                  {node.requireApproval && " · pede aprovação a cada chamada"}
                </p>

                {isEditing ? (
                  <form
                    className="settings-confirm devices-confirm"
                    onSubmit={(e) => {
                      e.preventDefault();
                      void save();
                    }}
                  >
                    <label className="settings-check">
                      <input type="checkbox" checked={editing.draft.enabled} onChange={(e) => patch({ enabled: e.target.checked })} />
                      Liberar para os agentes
                    </label>
                    <span className="field-hint">Agentes que podem usar (nenhum marcado = todos):</span>
                    {agentIds.map((id) => (
                      <label key={id} className="settings-check">
                        <input
                          type="checkbox"
                          checked={editing.draft.agents.includes(id)}
                          onChange={(e) =>
                            patch({ agents: e.target.checked ? [...editing.draft.agents, id] : editing.draft.agents.filter((a) => a !== id) })
                          }
                        />
                        {id}
                      </label>
                    ))}
                    <label className="settings-check">
                      <input type="checkbox" checked={editing.draft.requireApproval} onChange={(e) => patch({ requireApproval: e.target.checked })} />
                      Pedir minha aprovação a cada comando ou arquivo (tarefas agendadas não conseguem usar)
                    </label>
                    <label className="settings-field">
                      Chave de pareamento do hub
                      <input type="password" autoComplete="current-password" value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
                    </label>
                    {keyError && <p className="error-banner">{keyError}</p>}
                    <div className="skills-actions">
                      <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
                        {busy ? "Aguarde…" : "Salvar"}
                      </button>
                      <button type="button" className="link-button" disabled={busy} onClick={cancel}>
                        Cancelar
                      </button>
                    </div>
                  </form>
                ) : (
                  <div className="skills-actions">
                    <button
                      type="button"
                      className="link-button"
                      disabled={!conn || editing !== null}
                      onClick={() =>
                        setEditing({ deviceId: node.deviceId, draft: { enabled: node.enabled, agents: node.agents, requireApproval: node.requireApproval } })
                      }
                    >
                      Configurar acesso
                    </button>
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
