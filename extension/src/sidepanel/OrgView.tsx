import { useEffect, useState } from "react";
import type { AgentInfo, AgentTask, HubAgents, ModelPolicy, OrgEdit } from "../protocol/messages";
import type { AgentTasksResponse, HubAgentsResponse } from "../background/popup_protocol";
import { activityLine, activityOf } from "./lib/agentTasks";
import { addReportEdit, buildOrg, moveEdit, positionEdit, superiorChoices, type OrgNode } from "./lib/org";
import { delegationCandidates, delegationSummary, limitEdit, limitModels, nextPolicyId, policiesEdit, policiesWith, policiesWithout } from "./lib/modelPolicies";
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
type Painel =
  | { kind: "edit"; id: string }
  | { kind: "add"; under: string | null }
  | { kind: "remove"; id: string }
  /** Os modelos que um agente pode escolher ao delegar. */
  | { kind: "limit"; id: string }
  /** Uma política de modelo: `id` é a que se edita, ou `null` para uma nova. */
  | { kind: "policy"; id: string | null };

interface Edicao {
  onOpenChat: (id: string) => void;
  onOpenTasks: (id: string) => void;
  agents: AgentInfo[];
  /** Provedores, combos e políticas: do que um agente pode ser limitado. */
  candidates: string[];
  /** Provedores e combos: o que uma política pode responder. */
  modelIds: string[];
  policies: ModelPolicy[];
  painel: Painel | null;
  ocupado: boolean;
  abrir: (painel: Painel | null) => void;
  /** Pede a chave de pareamento para a mudança. */
  aplicar: (edit: OrgEdit) => void;
  /** O agente cujo cartão está sendo arrastado (P120): outro cartão, ou o topo, aceita soltá-lo se a mudança é válida. */
  arrastando: string | null;
  arrastar: (id: string | null) => void;
  /** O que o agente andou fazendo, numa linha, tirado das tarefas do hub; `null` sem nenhuma tarefa dele. */
  atividade: (id: string) => string | null;
}

/** Soltar o cartão arrastado sobre `alvo` (`null`: o topo): a mudança, se houver. Um cartão que vira filho de um descendente fecharia um círculo. */
function aoSoltar(edicao: Edicao, alvo: string | null): void {
  const edit = edicao.arrastando === null ? null : moveEdit(edicao.agents, edicao.arrastando, alvo);
  edicao.arrastar(null);
  if (edit) edicao.aplicar(edit);
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

/** Os modelos que o agente pode escolher ao delegar (P123): marcados, com um padrão (o primeiro). Nenhum marcado deixa a escolha aberta. */
function FormularioLimite({ agent, edicao }: { agent: AgentInfo; edicao: Edicao }) {
  const [marcados, setMarcados] = useState(() => new Set(agent.delegationModels));
  const [padrao, setPadrao] = useState(agent.delegationModels[0] ?? "");
  const escolhidos = edicao.candidates.filter((id) => marcados.has(id));
  const padraoValido = escolhidos.includes(padrao) ? padrao : (escolhidos[0] ?? "");
  function alternar(id: string) {
    const proximo = new Set(marcados);
    if (proximo.has(id)) proximo.delete(id);
    else proximo.add(id);
    setMarcados(proximo);
  }
  return (
    <div className="connection-form org-form">
      <p className="skills-hint">Modelos que {agent.id} pode escolher para as tarefas que delega. Nenhum marcado deixa a escolha aberta; com um só, ele dita o modelo.</p>
      {edicao.candidates.length === 0 && <p className="skills-hint">O hub não tem provedores nem políticas para escolher.</p>}
      {edicao.candidates.map((id) => (
        <label key={id} className="checkbox-label">
          <input type="checkbox" checked={marcados.has(id)} onChange={() => alternar(id)} />
          {id}
          {edicao.policies.some((p) => p.id === id) && <span className="skills-hint"> (política)</span>}
        </label>
      ))}
      {escolhidos.length > 1 && (
        <label>
          Padrão (o que uma tarefa recebe quando {agent.id} não escolhe)
          <select value={padraoValido} onChange={(e) => setPadrao(e.target.value)}>
            {escolhidos.map((id) => (
              <option key={id} value={id}>
                {id}
              </option>
            ))}
          </select>
        </label>
      )}
      <div className="skills-actions">
        <button type="button" disabled={edicao.ocupado} onClick={() => edicao.aplicar(limitEdit(agent.id, limitModels(edicao.candidates, marcados, padraoValido)))}>
          Salvar
        </button>
        <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir(null)}>
          Cancelar
        </button>
      </div>
    </div>
  );
}

/** Uma política de modelo (P123): um nome que o agente que delega pode dizer, o provedor ou combo que responde por ele, e quando escolher. */
function FormularioPolitica({ original, edicao }: { original: ModelPolicy | null; edicao: Edicao }) {
  const [id, setId] = useState(original?.id ?? nextPolicyId(edicao.candidates));
  const [modelo, setModelo] = useState(original?.model ?? edicao.modelIds[0] ?? "");
  const [descricao, setDescricao] = useState(original?.description ?? "");
  const pronto = id.trim() !== "" && modelo !== "";
  return (
    <div className="connection-form org-form">
      <label>
        Nome
        <input type="text" value={id} maxLength={64} onChange={(e) => setId(e.target.value)} />
      </label>
      <label>
        Quem responde
        <select value={modelo} onChange={(e) => setModelo(e.target.value)}>
          {edicao.modelIds.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
      </label>
      <label>
        Quando escolher (uma linha)
        <input type="text" value={descricao} maxLength={200} placeholder="ex.: trabalho simples e barato" onChange={(e) => setDescricao(e.target.value)} />
      </label>
      <div className="skills-actions">
        <button
          type="button"
          disabled={edicao.ocupado || !pronto}
          onClick={() => edicao.aplicar(policiesEdit(policiesWith(edicao.policies, { id, model: modelo, description: descricao }, original?.id ?? null)))}
        >
          Salvar política
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
  const alvoValido = edicao.arrastando !== null && moveEdit(edicao.agents, edicao.arrastando, agent.id) !== null;
  return (
    <li className="org-node">
      {/* O arrastar fica no cartão, não no <li>: o `dragover` dos subordinados não deve subir para o cartão do pai. */}
      <div
        className={`org-card${edicao.arrastando === agent.id ? " org-card--arrastando" : ""}${alvoValido ? " org-card--alvo" : ""}`}
        draggable={!edicao.ocupado}
        onDragStart={(e) => {
          e.dataTransfer.setData("text/plain", agent.id);
          e.dataTransfer.effectAllowed = "move";
          edicao.arrastar(agent.id);
        }}
        onDragEnd={() => edicao.arrastar(null)}
        onDragOver={(e) => {
          if (alvoValido) e.preventDefault();
        }}
        onDrop={(e) => {
          e.preventDefault();
          aoSoltar(edicao, agent.id);
        }}
      >
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
        {edicao.atividade(agent.id) && <span className="skills-hint org-limit">{edicao.atividade(agent.id)}</span>}
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
          {agent.canDelegateToAgents && (
            <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir({ kind: "limit", id: agent.id })} title={`Os modelos que ${agent.id} pode escolher ao delegar`}>
              Modelos
            </button>
          )}
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
      {painel?.kind === "limit" && painel.id === agent.id && <FormularioLimite key={`limit-${agent.id}`} agent={agent} edicao={edicao} />}
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
 * políticas de modelo e o limite de modelos de cada agente (P123) também se editam aqui, pela mesma operação estreita, sem o formulário inteiro. */
export default function OrgView({ onOpenChat, onOpenTasks }: { onOpenChat: (id: string) => void; onOpenTasks: (id: string) => void }) {
  const [hub, setHub] = useState<HubAgents | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [painel, setPainel] = useState<Painel | null>(null);
  const [asking, setAsking] = useState<OrgEdit | null>(null);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [arrastando, setArrastando] = useState<string | null>(null);
  const [tasks, setTasks] = useState<AgentTask[]>([]);

  function load() {
    chrome.runtime.sendMessage({ type: "listHubAgents" }).then((res: HubAgentsResponse) => {
      if (res.ok && res.hub) {
        setHub(res.hub);
        setError(null);
      } else {
        setError(res.error ?? "falha ao ler os agentes");
      }
    });
    // A atividade de cada nó é um extra: sem as tarefas, o cartão só não mostra a linha.
    chrome.runtime.sendMessage({ type: "listAgentTasks" }).then((res: AgentTasksResponse) => {
      if (res.ok) setTasks(res.tasks);
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

  const { agents, modelPolicies, modelIds } = hub;
  const edicao: Edicao = {
    onOpenChat,
    onOpenTasks,
    agents,
    candidates: delegationCandidates(modelIds, modelPolicies),
    modelIds,
    policies: modelPolicies,
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
    arrastando,
    arrastar: setArrastando,
    atividade: (id) => {
      const activity = activityOf(tasks, id);
      return activity ? activityLine(activity, Date.now()) : null;
    },
  };
  const topoValido = arrastando !== null && moveEdit(agents, arrastando, null) !== null;
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
      {topoValido && (
        <div
          className="org-topo"
          onDragOver={(e) => e.preventDefault()}
          onDrop={(e) => {
            e.preventDefault();
            aoSoltar(edicao, null);
          }}
        >
          Soltar aqui para tirar de baixo do superior (topo da árvore)
        </div>
      )}
      {tree.length > 0 && ninguemReporta && <p className="skills-hint">Ninguém reporta a ninguém ainda: arraste um cartão para cima de outro, ou use "Editar" num agente para escolher o superior dele.</p>}
      {painel?.kind === "add" && painel.under === null && <FormularioNovo key="add-top" under={null} edicao={edicao} />}
      <div className="skills-actions">
        <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir({ kind: "add", under: null })}>
          Adicionar um agente no topo
        </button>
      </div>
      <h2 className="agents-subtitle">Políticas de modelo</h2>
      {modelPolicies.length === 0 ? (
        <p className="skills-hint">Nenhuma política: um agente que delega vê só os ids dos modelos.</p>
      ) : (
        <ul className="skills-list">
          {modelPolicies.map((policy) => (
            <li key={policy.id} className="skills-item">
              <span className="skills-item-name">{policy.id}</span> <span className="skills-hint">→ {policy.model}</span>
              {policy.description && <p className="skills-item-description">{policy.description}</p>}
              <span className="org-actions">
                <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir({ kind: "policy", id: policy.id })}>
                  Editar
                </button>
                <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.aplicar(policiesEdit(policiesWithout(modelPolicies, policy.id)))}>
                  Remover
                </button>
              </span>
              {painel?.kind === "policy" && painel.id === policy.id && <FormularioPolitica key={`policy-${policy.id}`} original={policy} edicao={edicao} />}
            </li>
          ))}
        </ul>
      )}
      {painel?.kind === "policy" && painel.id === null && <FormularioPolitica key="policy-new" original={null} edicao={edicao} />}
      <div className="skills-actions">
        <button type="button" className="link-button" disabled={edicao.ocupado || modelIds.length === 0} onClick={() => edicao.abrir({ kind: "policy", id: null })}>
          Nova política
        </button>
      </div>
    </div>
  );
}
