/** P120 — a organização dos agentes: cada um pode ter um cargo e um superior (`reportsTo`, o id de outro agente).
 * Por enquanto só aparece: não muda nada do que um agente pode fazer. Espelha `warden_bootstrap::org`. */
export interface OrgAgent {
  id: string;
  role?: string | null;
  reportsTo?: string | null;
}

export interface OrgNode<T extends OrgAgent = OrgAgent> {
  agent: T;
  children: OrgNode<T>[];
}

/** Os agentes como uma floresta: quem não tem superior fica no topo, na ordem em que vieram, com os seus subordinados
 * embaixo. Quem tem o superior sumido aparece no topo em vez de se perder, e um ciclo (que o hub recusa) nunca trava a árvore. */
export function buildOrg<T extends OrgAgent>(agents: T[]): OrgNode<T>[] {
  const ids = new Set(agents.map((a) => a.id));
  const seen = new Set<string>();
  const nodeOf = (agent: T): OrgNode<T> => {
    seen.add(agent.id);
    const children: OrgNode<T>[] = [];
    for (const report of agents) {
      if (report.reportsTo === agent.id && !seen.has(report.id)) children.push(nodeOf(report));
    }
    return { agent, children };
  };
  return agents.filter((a) => !a.reportsTo || !ids.has(a.reportsTo)).map(nodeOf);
}

/** Todos abaixo de `id`, em qualquer nível: a quem ele não pode passar a reportar. */
export function descendantsOf(agents: OrgAgent[], id: string): Set<string> {
  const below = new Set<string>();
  const queue = [id];
  while (queue.length > 0) {
    const current = queue.pop()!;
    for (const a of agents) {
      if (a.reportsTo === current && !below.has(a.id)) {
        below.add(a.id);
        queue.push(a.id);
      }
    }
  }
  return below;
}

/** A quem `id` pode passar a reportar: todos menos ele mesmo e quem está abaixo dele (fecharia um ciclo). */
export function superiorChoices<T extends OrgAgent>(agents: T[], id: string): T[] {
  const below = descendantsOf(agents, id);
  return agents.filter((a) => a.id !== id && !below.has(a.id));
}

/** Uma mudança feita pela árvore, como o hub a recebe (`AgentOrgEdit`). `role` e `reportsTo` ausentes querem dizer nenhum. */
export type OrgEdit =
  | { kind: "setPosition"; id: string; role?: string; reportsTo?: string }
  | { kind: "addReport"; id: string; persona: string; role?: string; reportsTo?: string }
  | { kind: "remove"; id: string }
  /** P123 — o que os clientes leves (celular e extensão) mandam; a web edita o limite e as políticas pelo formulário de Configurações. */
  | { kind: "setDelegationModels"; id: string; models: string[] }
  | { kind: "setModelPolicies"; policies: Array<{ id: string; model: string; description: string }> };

/** Um campo em branco é nenhum valor: a mudança o deixa de fora em vez de mandar uma string vazia. */
export function positionEdit(id: string, role: string, reportsTo: string): OrgEdit {
  return { kind: "setPosition", id, ...(role.trim() ? { role: role.trim() } : {}), ...(reportsTo ? { reportsTo } : {}) };
}

export function addReportEdit(id: string, persona: string, role: string, reportsTo: string | null): OrgEdit {
  return { kind: "addReport", id: id.trim(), persona: persona.trim(), ...(role.trim() ? { role: role.trim() } : {}), ...(reportsTo ? { reportsTo } : {}) };
}

/** Soltar o cartão de `draggedId` sobre o de `targetId` (ou `null`: o topo da árvore): a mudança de posição que o hub recebe, com o cargo que
 * ele já tem. `null` quando não há o que mudar: o agente não existe, o alvo é ele mesmo ou alguém abaixo dele (fecharia um ciclo, o hub também
 * recusa) ou o superior de hoje já é o alvo. */
export function moveEdit(agents: OrgAgent[], draggedId: string, targetId: string | null): OrgEdit | null {
  const dragged = agents.find((a) => a.id === draggedId);
  if (!dragged) return null;
  if (targetId !== null && !superiorChoices(agents, draggedId).some((a) => a.id === targetId)) return null;
  if ((dragged.reportsTo ?? null) === targetId) return null;
  return positionEdit(draggedId, dragged.role ?? "", targetId ?? "");
}

/** Um agente foi renomeado: quem reportava a ele acompanha o nome novo. */
export function renameInReports<T extends OrgAgent>(agents: T[], from: string, to: string): T[] {
  if (from === to) return agents;
  return agents.map((a) => (a.reportsTo === from ? { ...a, reportsTo: to } : a));
}

/** `removed` está saindo: quem reportava a ele passa a reportar ao superior dele (ou a ninguém). A lista volta sem ele. */
export function removeFromOrg<T extends OrgAgent>(agents: T[], removed: string): T[] {
  const superior = agents.find((a) => a.id === removed)?.reportsTo ?? null;
  return agents.filter((a) => a.id !== removed).map((a) => (a.reportsTo === removed ? { ...a, reportsTo: superior } : a));
}

/** O que um membro faz com a organização dos agentes (P120): o dono escolhe por membro. A árvore é uma só, a do dono. */
export type OrgAccess = "none" | "view" | "edit";

/** O acesso que o hub mandou (`orgAccess` do membro, ausente quando é nenhum); um valor que não conhecemos vale como nenhum, para não
 * mostrar uma tela que o hub vai recusar. */
export function orgAccessOf(value: string | null | undefined): OrgAccess {
  return value === "view" || value === "edit" ? value : "none";
}

/** Os três níveis, na ordem em que o dono os escolhe. */
export const ORG_ACCESS_CHOICES: Array<{ value: OrgAccess; label: string; hint: string }> = [
  { value: "none", label: "Não vê", hint: "Não vê a árvore, como todo membro antes disso." },
  { value: "view", label: "Só vê", hint: "Vê quem reporta a quem e o cargo de cada agente; não vê o que eles dizem nem o que podem fazer." },
  { value: "edit", label: "Vê e edita", hint: "Muda cargo e superior, adiciona e remove agentes, com as mesmas regras de uma mudança sua, sem a chave de pareamento." },
];

/** A frase curta do cartão do membro. */
export function orgAccessLabel(access: OrgAccess): string {
  return access === "edit" ? "organograma: vê e edita" : access === "view" ? "organograma: só vê" : "organograma: não vê";
}
