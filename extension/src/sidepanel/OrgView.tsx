import { useEffect, useState } from "react";
import type { AgentInfo, HubAgents, OrgEdit } from "../protocol/messages";
import type { HubAgentsResponse } from "../background/popup_protocol";
import { addReportEdit, buildOrg, positionEdit, superiorChoices, type OrgNode } from "./lib/org";
import { delegationSummary } from "./lib/modelPolicies";
import PairingKeyForm from "./PairingKeyForm";

const AUTONOMIA: Record<number, string> = { 1: "só responde", 2: "sugere", 3: "pede antes", 4: "age sozinho" };

/** O que o agente pode fazer, como a tela de Configurações define. */
function selos(agent: AgentInfo): string[] {
  const lista: string[] = [];
  if (agent.canDelegateToAgents) lista.push("delega");
  if (agent.canManageAgents) lista.push("gerencia agentes");
  if (agent.canMessageAgents) lista.push("deixa recados");
  if (agent.canManageTasks) lista.push("agenda tarefas");
  if (agent.autonomy !== 4) lista.push(`autonomia ${agent.autonomy}: ${AUTONOMIA[agent.autonomy] ?? ""}`.trim());
  if (agent.approvalRequired.length > 0) lista.push(`pede antes: ${agent.approvalRequired.length} tipo${agent.approvalRequired.length > 1 ? "s" : ""} de ação`);
  return lista;
}

/** O que está aberto na árvore: o cargo de um agente, um subordinado novo sob um agente (`null`: no topo) ou a remoção de um. */
type Painel = { kind: "edit"; id: string } | { kind: "add"; under: string | null } | { kind: "remove"; id: string };

interface Edicao {
  onOpenChat: (id: string) => void;
  onOpenTasks: (id: string) => void;
  agents: AgentInfo[];
  painel: Painel | null;
  ocupado: boolean;
  abrir: (painel: Painel | null) => void;
  /** Pede a chave de pareamento para a mudança. */
  aplicar: (edit: OrgEdit) => void;
}

function FormularioCargo({ agent, edicao }: { agent: AgentInfo; edicao: Edicao }) {
  const [cargo, setCargo] = useState(agent.role ?? "");
  const [superior, setSuperior] = useState(agent.reportsTo ?? "");
  const opcoes = superiorChoices(edicao.agents, agent.id);
  return (
    <div className="connection-form org-form">
      <label>
        Cargo
        <input type="text" value={cargo} maxLength={80} placeholder="ex.: líder de backend" onChange={(e) => setCargo(e.target.value)} />
      </label>
      <label>
        Reporta a
        <select value={superior} onChange={(e) => setSuperior(e.target.value)}>
          <option value="">Ninguém (topo da árvore)</option>
          {opcoes.map((a) => (
            <option key={a.id} value={a.id}>
              {a.id}
            </option>
          ))}
        </select>
        <span className="skills-hint">Quem reporta a {agent.id} vai junto.</span>
      </label>
      <div className="skills-actions">
        <button type="button" disabled={edicao.ocupado} onClick={() => edicao.aplicar(positionEdit(agent.id, cargo, superior))}>
          Salvar
        </button>
        <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir(null)}>
          Cancelar
        </button>
      </div>
    </div>
  );
}

function FormularioNovo({ under, edicao }: { under: string | null; edicao: Edicao }) {
  const [id, setId] = useState("");
  const [cargo, setCargo] = useState("");
  const [persona, setPersona] = useState("");
  const pronto = id.trim() !== "" && persona.trim() !== "";
  return (
    <div className="connection-form org-form">
      <label>
        Nome
        <input type="text" value={id} maxLength={64} placeholder="ex.: revisor" onChange={(e) => setId(e.target.value)} />
      </label>
      <label>
        Cargo
        <input type="text" value={cargo} maxLength={80} placeholder="opcional" onChange={(e) => setCargo(e.target.value)} />
      </label>
      <label>
        O que ele faz
        <textarea value={persona} rows={3} placeholder="As instruções dele, em poucas linhas." onChange={(e) => setPersona(e.target.value)} />
        <span className="skills-hint">
          {under ? `Reporta a ${under}. ` : "Começa no topo da árvore. "}
          Começa cuidadoso: ferramentas só de leitura, pede antes de qualquer mudança, e não delega nem gerencia agentes até alguém ligar isso nas configurações.
        </span>
      </label>
      <div className="skills-actions">
        <button type="button" disabled={edicao.ocupado || !pronto} onClick={() => edicao.aplicar(addReportEdit(id, persona, cargo, under))}>
          Adicionar agente
        </button>
        <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir(null)}>
          Cancelar
        </button>
      </div>
    </div>
  );
}

function ConfirmarRemocao({ node, edicao }: { node: OrgNode<AgentInfo>; edicao: Edicao }) {
  const { agent, children } = node;
  return (
    <div className="connection-form org-form">
      <p className="skills-hint">
        Remover <strong>{agent.id}</strong>?
        {children.length > 0 && ` ${children.length === 1 ? "O subordinado dele passa" : `Os ${children.length} subordinados dele passam`} a reportar a ${agent.reportsTo ?? "ninguém (o topo da árvore)"}.`}{" "}
        As conversas ficam; não dá para desfazer.
      </p>
      <div className="skills-actions">
        <button type="button" disabled={edicao.ocupado} onClick={() => edicao.aplicar({ kind: "remove", id: agent.id })}>
          Remover
        </button>
        <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir(null)}>
          Manter
        </button>
      </div>
    </div>
  );
}

function Node({ node, edicao }: { node: OrgNode<AgentInfo>; edicao: Edicao }) {
  const { agent, children } = node;
  const { painel } = edicao;
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
        {agent.canDelegateToAgents && <span className="skills-hint org-limit">{delegationSummary(agent.delegationModels)}</span>}
        {children.length > 0 && <span className="org-count">{children.length === 1 ? "1 subordinado" : `${children.length} subordinados`}</span>}
        <span className="org-actions">
          <button type="button" className="link-button" onClick={() => edicao.onOpenChat(agent.id)} title={`Começar uma conversa com ${agent.id}`}>
            Conversar
          </button>
          <button type="button" className="link-button" onClick={() => edicao.onOpenTasks(agent.id)} title={`As tarefas que ${agent.id} recebeu e as que delegou`}>
            Tarefas
          </button>
          <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir({ kind: "edit", id: agent.id })}>
            Editar
          </button>
          <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir({ kind: "add", under: agent.id })}>
            Adicionar subordinado
          </button>
          <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir({ kind: "remove", id: agent.id })}>
            Remover
          </button>
        </span>
      </div>
      {painel?.kind === "edit" && painel.id === agent.id && <FormularioCargo key={`edit-${agent.id}`} agent={agent} edicao={edicao} />}
      {painel?.kind === "remove" && painel.id === agent.id && <ConfirmarRemocao node={node} edicao={edicao} />}
      {painel?.kind === "add" && painel.under === agent.id && <FormularioNovo key={`add-${agent.id}`} under={agent.id} edicao={edicao} />}
      {children.length > 0 && (
        <ul className="org-children">
          {children.map((child) => (
            <Node key={child.agent.id} node={child} edicao={edicao} />
          ))}
        </ul>
      )}
    </li>
  );
}

/** P120, P123 — quem reporta a quem entre os agentes, e onde isso se muda: dar um cargo a um agente, passá-lo para baixo de outro, adicionar um
 * subordinado, remover um. Cada mudança é escrita sozinha (nada mais das configurações é tocado) e pede a chave de pareamento do hub. As
 * políticas de modelo e o limite de modelos de cada agente só aparecem aqui: quem os edita é o desktop ou a web. */
export default function OrgView({ onOpenChat, onOpenTasks }: { onOpenChat: (id: string) => void; onOpenTasks: (id: string) => void }) {
  const [hub, setHub] = useState<HubAgents | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [painel, setPainel] = useState<Painel | null>(null);
  const [asking, setAsking] = useState<OrgEdit | null>(null);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  function load() {
    chrome.runtime.sendMessage({ type: "listHubAgents" }).then((res: HubAgentsResponse) => {
      if (res.ok && res.hub) {
        setHub(res.hub);
        setError(null);
      } else {
        setError(res.error ?? "falha ao ler os agentes");
      }
    });
  }

  useEffect(load, []);

  function cancelKey() {
    setAsking(null);
    setKeyError(null);
  }

  function confirm(pairingKey: string) {
    if (!asking) return;
    setBusy(true);
    setKeyError(null);
    chrome.runtime.sendMessage({ type: "editAgentOrg", pairingKey, edit: asking }).then((res: HubAgentsResponse) => {
      setBusy(false);
      if (res.ok && res.hub) {
        setHub(res.hub);
        setPainel(null);
        setError(null);
        cancelKey();
      } else if (res.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        cancelKey();
        setError(res.error ?? "falha ao mudar a organização");
      }
    });
  }

  if (hub === null) {
    return <div className="agents-pane">{error ? <p className="error-banner">{error}</p> : <p className="skills-hint">Carregando…</p>}</div>;
  }

  const { agents, modelPolicies } = hub;
  const edicao: Edicao = {
    onOpenChat,
    onOpenTasks,
    agents,
    painel,
    ocupado: busy || asking !== null,
    abrir: (next) => {
      setError(null);
      setPainel(next);
    },
    aplicar: (edit) => {
      setError(null);
      setKeyError(null);
      setAsking(edit);
    },
  };
  const tree = buildOrg(agents);
  const ninguemReporta = tree.every((node) => node.children.length === 0);

  return (
    <div className="agents-pane">
      <div className="skills-toolbar">
        <span className="skills-hint">
          Quem reporta a quem entre os agentes. Quem gerencia ou delega a outros agentes alcança só os que estão abaixo dele. Toda mudança feita por um agente ainda espera o seu sim.
        </span>
        <button type="button" className="link-button" onClick={load} disabled={busy}>
          Atualizar
        </button>
      </div>
      {error && <p className="error-banner">{error}</p>}
      {asking && (
        <PairingKeyForm busy={busy} error={keyError} onConfirm={confirm} onCancel={cancelKey}>
          A chave é pedida a cada mudança, e o hub reinicia com ela.
        </PairingKeyForm>
      )}
      {tree.length === 0 ? (
        <p className="skills-hint">Nenhum agente ainda.</p>
      ) : (
        <ul className="org-tree">
          {tree.map((node) => (
            <Node key={node.agent.id} node={node} edicao={edicao} />
          ))}
        </ul>
      )}
      {tree.length > 0 && ninguemReporta && <p className="skills-hint">Ninguém reporta a ninguém ainda: use "Editar" num agente para escolher o superior dele.</p>}
      {painel?.kind === "add" && painel.under === null && <FormularioNovo key="add-top" under={null} edicao={edicao} />}
      <div className="skills-actions">
        <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir({ kind: "add", under: null })}>
          Adicionar um agente no topo
        </button>
      </div>
      <h2 className="agents-subtitle">Políticas de modelo</h2>
      {modelPolicies.length === 0 ? (
        <p className="skills-hint">Nenhuma política: um agente que delega vê só os ids dos modelos. Elas se criam no desktop ou na web.</p>
      ) : (
        <ul className="skills-list">
          {modelPolicies.map((policy) => (
            <li key={policy.id} className="skills-item">
              <span className="skills-item-name">{policy.id}</span> <span className="skills-hint">→ {policy.model}</span>
              {policy.description && <p className="skills-item-description">{policy.description}</p>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
