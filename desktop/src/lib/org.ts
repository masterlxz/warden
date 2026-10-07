/** P120 — the organization of the agents: each may have a role and a superior (`reportsTo`, an agent id). Only shown
 * for now; it changes nothing an agent may do. Mirrors `warden_bootstrap::org`. */
export interface OrgAgent {
  id: string;
  role?: string | null;
  reportsTo?: string | null;
}

export interface OrgNode<T extends OrgAgent = OrgAgent> {
  agent: T;
  children: OrgNode<T>[];
}

/** The agents as a forest: the ones with no superior at the top, in the order they came, each with its reports
 * under it. An agent whose superior is gone is shown at the top rather than lost, and a circle (which the save
 * refuses) never loops the tree. */
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

/** Every agent below `id`, at any depth: the ones it can't be made to report to. */
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

/** Who `id` can be made to report to: everyone but itself and the agents below it (that would close a circle). */
export function superiorChoices<T extends OrgAgent>(agents: T[], id: string): T[] {
  const below = descendantsOf(agents, id);
  return agents.filter((a) => a.id !== id && !below.has(a.id));
}

/** One change made from the tree, as the app and the hub take it (`AgentOrgEdit`). `role` and `reportsTo` left out mean none. */
export type OrgEdit =
  | { kind: "setPosition"; id: string; role?: string; reportsTo?: string }
  | { kind: "addReport"; id: string; persona: string; role?: string; reportsTo?: string }
  | { kind: "remove"; id: string };

/** A blank field is no value: the edit leaves it out instead of sending an empty string. */
export function positionEdit(id: string, role: string, reportsTo: string): OrgEdit {
  return { kind: "setPosition", id, ...(role.trim() ? { role: role.trim() } : {}), ...(reportsTo ? { reportsTo } : {}) };
}

export function addReportEdit(id: string, persona: string, role: string, reportsTo: string | null): OrgEdit {
  return { kind: "addReport", id: id.trim(), persona: persona.trim(), ...(role.trim() ? { role: role.trim() } : {}), ...(reportsTo ? { reportsTo } : {}) };
}

/** An agent was renamed: whoever reported to it follows the new name. */
export function renameInReports<T extends OrgAgent>(agents: T[], from: string, to: string): T[] {
  if (from === to) return agents;
  return agents.map((a) => (a.reportsTo === from ? { ...a, reportsTo: to } : a));
}

/** `removed` is leaving: whoever reported to it reports to its superior instead (or to nobody). The list comes back without it. */
export function removeFromOrg<T extends OrgAgent>(agents: T[], removed: string): T[] {
  const superior = agents.find((a) => a.id === removed)?.reportsTo ?? null;
  return agents.filter((a) => a.id !== removed).map((a) => (a.reportsTo === removed ? { ...a, reportsTo: superior } : a));
}
