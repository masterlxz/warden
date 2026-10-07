// P123 — the models an agent may delegate with, and the named policies ("fast", "reasoning"...) over them, as the Settings form edits
// them. Pure, so the cascades (a rename or a removal reaching whatever names the model) can be tested without a window.

import type { AgentEntry, ModelPolicy } from "../types";

/** The part of the settings form these cascades read and change. */
export interface PolicyForm {
  agents: AgentEntry[];
  modelPolicies: ModelPolicy[];
}

/** Every id an agent may be limited to: the providers, then the combos, then the policies. */
export function delegationCandidates(providers: { id: string }[], combos: { id: string }[], policies: { id: string }[]): string[] {
  return [...providers, ...combos, ...policies].map((m) => m.id).filter((id) => id.trim() !== "");
}

/** A provider or a combo was renamed: the policies answered by it follow, and so does whatever agent listed it. */
export function renameModel<T extends PolicyForm>(form: T, from: string, to: string): T {
  if (from === to) return form;
  return {
    ...form,
    modelPolicies: form.modelPolicies.map((p) => (p.model === from ? { ...p, model: to } : p)),
    agents: form.agents.map((a) => (a.delegationModels?.includes(from) ? { ...a, delegationModels: a.delegationModels.map((m) => (m === from ? to : m)) } : a)),
  };
}

/** A policy was renamed: the agents that listed it follow. */
export function renamePolicy<T extends PolicyForm>(form: T, from: string, to: string): T {
  if (from === to) return form;
  return { ...form, agents: form.agents.map((a) => (a.delegationModels?.includes(from) ? { ...a, delegationModels: a.delegationModels.map((m) => (m === from ? to : m)) } : a)) };
}

/** A provider or combo is going: the policies it answered go too, and every agent's list forgets it and them. A list left empty leaves the
 * choice open again. */
export function dropModel<T extends PolicyForm>(form: T, id: string): T {
  const goneWith = form.modelPolicies.filter((p) => p.model === id).map((p) => p.id);
  const gone = new Set([id, ...goneWith]);
  return {
    ...form,
    modelPolicies: form.modelPolicies.filter((p) => p.model !== id),
    agents: form.agents.map((a) => (a.delegationModels?.some((m) => gone.has(m)) ? { ...a, delegationModels: a.delegationModels.filter((m) => !gone.has(m)) } : a)),
  };
}

/** A policy is going: the agents that listed it forget it. */
export function dropPolicy<T extends PolicyForm>(form: T, id: string): T {
  return {
    ...form,
    modelPolicies: form.modelPolicies.filter((p) => p.id !== id),
    agents: form.agents.map((a) => (a.delegationModels?.includes(id) ? { ...a, delegationModels: a.delegationModels.filter((m) => m !== id) } : a)),
  };
}

/** The name a new policy starts with: `policy-1`, `policy-2`... free among providers, combos and policies. */
export function nextPolicyId(taken: string[]): string {
  let n = 1;
  while (taken.includes(`policy-${n}`)) n += 1;
  return `policy-${n}`;
}

/** What the list says about an agent's models, in a sentence the card shows under it. */
export function delegationSummary(models: string[] | undefined): string {
  if (!models || models.length === 0) return "Open: the agent picks any model for each task it delegates.";
  if (models.length === 1) return `Dictated: every task it delegates runs on ${models[0]}.`;
  return `Limited to ${models.length} models; ${models[0]} is what a task gets when the agent names none.`;
}
