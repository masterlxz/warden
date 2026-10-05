import { useCallback, useEffect, useState } from "react";
import type { ServerConnection } from "../hub/connection";
import type { AgentSettings } from "../hub/messages";
import { approvalCategoryLabel } from "../hub/approvalCategories";
import { buildOrg, type OrgNode } from "../hub/org";

const AUTONOMIA: Record<number, string> = { 1: "só responde", 2: "sugere", 3: "pede antes", 4: "age sozinho" };

/** O que o agente pode fazer, como a tela de Configurações define. */
function selos(agent: AgentSettings): string[] {
  const lista: string[] = [];
  if (agent.canDelegateToAgents) lista.push("delega");
  if (agent.canManageAgents) lista.push("gerencia agentes");
  if (agent.canMessageAgents) lista.push("deixa recados");
  if (agent.canManageTasks) lista.push("agenda tarefas");
  if (agent.autonomy !== 4) lista.push(`autonomia ${agent.autonomy}: ${AUTONOMIA[agent.autonomy] ?? ""}`.trim());
  if (agent.approvalRequired.length > 0) lista.push(`pede antes: ${agent.approvalRequired.map(approvalCategoryLabel).join(", ").toLowerCase()}`);
  return lista;
}

function Node({ node }: { node: OrgNode<AgentSettings> }) {
  const { agent, children } = node;
  return (
    <li className="org-node">
      <div className="org-card">
        <span className="org-name">{agent.id}</span>
        {agent.role && <span className="org-role">{agent.role}</span>}
        <span className="org-badges">
          {selos(agent).map((selo) => (
            <span key={selo} className="org-badge">
              {selo}
            </span>
          ))}
        </span>
        {children.length > 0 && <span className="org-count">{children.length === 1 ? "1 subordinado" : `${children.length} subordinados`}</span>}
      </div>
      {children.length > 0 && (
        <ul className="org-children">
          {children.map((child) => (
            <Node key={child.agent.id} node={child} />
          ))}
        </ul>
      )}
    </li>
  );
}

/** P120 — quem reporta a quem entre os agentes, só leitura. O cargo e o superior se definem em cada agente, em
 * Configurações; por enquanto isto é só um retrato, e nada aqui muda o que um agente pode fazer. */
export default function OrganizationView({ conn, onEdit }: { conn: ServerConnection | null; onEdit: () => void }) {
  const [agents, setAgents] = useState<AgentSettings[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!conn) return;
    try {
      const loaded = await conn.requestSettings();
      setAgents(loaded.settings.agents);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [conn]);

  useEffect(() => {
    void load();
  }, [load]);

  if (agents === null) {
    return <div className="usage-view">{error ? <p className="error-banner">{error}</p> : <p className="skills-hint">Carregando…</p>}</div>;
  }

  const tree = buildOrg(agents);
  const ninguemReporta = tree.every((node) => node.children.length === 0);

  return (
    <div className="usage-view">
      <div className="skills-toolbar">
        <span className="skills-hint">Quem reporta a quem entre os seus agentes. É só um retrato por enquanto: não muda o que nenhum agente pode fazer.</span>
        <button type="button" className="link-button" onClick={() => void load()} disabled={!conn}>
          Atualizar
        </button>
      </div>
      {error && <p className="error-banner">{error}</p>}
      {tree.length === 0 ? (
        <p className="skills-hint">Nenhum agente ainda. Crie um em Configurações.</p>
      ) : (
        <ul className="org-tree">
          {tree.map((node) => (
            <Node key={node.agent.id} node={node} />
          ))}
        </ul>
      )}
      {tree.length > 0 && ninguemReporta && <p className="skills-hint">Ninguém reporta a ninguém ainda: escolha "Reporta a" num agente, em Configurações.</p>}
      <div>
        <button type="button" className="link-button" onClick={onEdit}>
          Editar agentes em Configurações
        </button>
      </div>
    </div>
  );
}
