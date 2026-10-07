import { useCallback, useEffect, useState } from "react";
import { SettingsError, type ServerConnection } from "../hub/connection";
import type { AgentSettings } from "../hub/messages";
import { approvalCategoryLabel } from "../hub/approvalCategories";
import { addReportEdit, buildOrg, positionEdit, superiorChoices, type OrgEdit, type OrgNode } from "../hub/org";

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

/** O que está aberto na árvore: o cargo de um agente, um subordinado novo sob um agente (`null`: no topo) ou a remoção de um. */
type Painel = { kind: "edit"; id: string } | { kind: "add"; under: string | null } | { kind: "remove"; id: string };

interface Edicao {
  /** Abre uma conversa nova com este agente, ou as tarefas que ele recebeu e delegou. */
  onOpenChat: (id: string) => void;
  onOpenTasks: (id: string) => void;
  agents: AgentSettings[];
  painel: Painel | null;
  ocupado: boolean;
  abrir: (painel: Painel | null) => void;
  /** Pede a chave de pareamento para a mudança. */
  aplicar: (edit: OrgEdit) => void;
}

function FormularioCargo({ agent, edicao }: { agent: AgentSettings; edicao: Edicao }) {
  const [cargo, setCargo] = useState(agent.role ?? "");
  const [superior, setSuperior] = useState(agent.reportsTo ?? "");
  const opcoes = superiorChoices(edicao.agents, agent.id);
  return (
    <div className="org-form">
      <label className="settings-field">
        Cargo
        <input type="text" value={cargo} maxLength={80} placeholder="ex.: líder de backend" onChange={(e) => setCargo(e.target.value)} />
      </label>
      <label className="settings-field">
        Reporta a
        <select value={superior} onChange={(e) => setSuperior(e.target.value)}>
          <option value="">Ninguém (topo da árvore)</option>
          {opcoes.map((a) => (
            <option key={a.id} value={a.id}>
              {a.id}
            </option>
          ))}
        </select>
        <span className="field-hint">Quem reporta a {agent.id} vai junto.</span>
      </label>
      <div className="skills-actions">
        <button type="button" className="primary-button" disabled={edicao.ocupado} onClick={() => edicao.aplicar(positionEdit(agent.id, cargo, superior))}>
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
    <div className="org-form">
      <label className="settings-field">
        Nome
        <input type="text" value={id} maxLength={64} placeholder="ex.: revisor" onChange={(e) => setId(e.target.value)} />
      </label>
      <label className="settings-field">
        Cargo
        <input type="text" value={cargo} maxLength={80} placeholder="opcional" onChange={(e) => setCargo(e.target.value)} />
      </label>
      <label className="settings-field">
        O que ele faz
        <textarea value={persona} rows={3} placeholder="As instruções dele, em poucas linhas." onChange={(e) => setPersona(e.target.value)} />
        <span className="field-hint">
          {under ? `Reporta a ${under}. ` : "Começa no topo da árvore. "}
          Começa cuidadoso: ferramentas só de leitura, pede antes de qualquer mudança, e não delega nem gerencia agentes até você ligar isso em Configurações.
        </span>
      </label>
      <div className="skills-actions">
        <button type="button" className="primary-button" disabled={edicao.ocupado || !pronto} onClick={() => edicao.aplicar(addReportEdit(id, persona, cargo, under))}>
          Adicionar agente
        </button>
        <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir(null)}>
          Cancelar
        </button>
      </div>
    </div>
  );
}

function ConfirmarRemocao({ node, edicao }: { node: OrgNode<AgentSettings>; edicao: Edicao }) {
  const { agent, children } = node;
  return (
    <div className="org-form">
      <p className="skills-hint">
        Remover <strong>{agent.id}</strong>?
        {children.length > 0 && ` ${children.length === 1 ? "O subordinado dele passa" : `Os ${children.length} subordinados dele passam`} a reportar a ${agent.reportsTo ?? "ninguém (o topo da árvore)"}.`}
        {" "}As conversas ficam; não dá para desfazer.
      </p>
      <div className="skills-actions">
        <button type="button" className="primary-button" disabled={edicao.ocupado} onClick={() => edicao.aplicar({ kind: "remove", id: agent.id })}>
          Remover
        </button>
        <button type="button" className="link-button" disabled={edicao.ocupado} onClick={() => edicao.abrir(null)}>
          Manter
        </button>
      </div>
    </div>
  );
}

function Node({ node, edicao }: { node: OrgNode<AgentSettings>; edicao: Edicao }) {
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

/** P120 — quem reporta a quem entre os agentes, e onde isso se muda: dar um cargo a um agente, passá-lo para baixo de outro, adicionar um
 * subordinado, remover um. Cada mudança é escrita sozinha (nada mais das configurações é tocado) e pede a chave de pareamento do hub; quem
 * gerencia (`manage_agents`) ou delega (`delegate_to_agent`) alcança só os que estão abaixo dele, então a árvore define o alcance. */
export default function OrganizationView({
  conn,
  onEdit,
  onOpenChat,
  onOpenTasks,
}: {
  conn: ServerConnection | null;
  onEdit: () => void;
  onOpenChat: (id: string) => void;
  onOpenTasks: (id: string) => void;
}) {
  const [agents, setAgents] = useState<AgentSettings[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [painel, setPainel] = useState<Painel | null>(null);
  const [asking, setAsking] = useState<OrgEdit | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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

  function cancelKey() {
    setAsking(null);
    setPairingKey("");
    setKeyError(null);
  }

  async function confirm() {
    if (!conn || !asking) return;
    setBusy(true);
    setKeyError(null);
    try {
      setAgents(await conn.editAgentOrg(pairingKey, asking));
      setPainel(null);
      setError(null);
      cancelKey();
    } catch (err) {
      if (err instanceof SettingsError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        cancelKey();
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setBusy(false);
    }
  }

  if (agents === null) {
    return <div className="usage-view">{error ? <p className="error-banner">{error}</p> : <p className="skills-hint">Carregando…</p>}</div>;
  }

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

  const keyPrompt = asking && (
    <form
      className="settings-confirm devices-confirm"
      onSubmit={(e) => {
        e.preventDefault();
        void confirm();
      }}
    >
      <label className="settings-field">
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        <span className="field-hint">A mesma do primeiro login. É pedida a cada mudança, e o hub reinicia com ela.</span>
      </label>
      {keyError && <p className="error-banner">{keyError}</p>}
      <div className="skills-actions">
        <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : "Confirmar"}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancelKey}>
          Cancelar
        </button>
      </div>
    </form>
  );

  return (
    <div className="usage-view">
      <div className="skills-toolbar">
        <span className="skills-hint">Quem reporta a quem entre os seus agentes. Quem gerencia ou delega a outros agentes alcança só os que estão abaixo dele; quem está fora da hierarquia delega como antes. Toda mudança feita por um agente ainda espera o seu sim.</span>
        <button type="button" className="link-button" onClick={() => void load()} disabled={!conn || busy}>
          Atualizar
        </button>
      </div>
      {error && <p className="error-banner">{error}</p>}
      {keyPrompt}
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
        <button type="button" className="link-button" onClick={onEdit}>
          Mais configurações por agente
        </button>
      </div>
    </div>
  );
}
