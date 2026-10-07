// P123 — os modelos com que um agente pode delegar, e as políticas nomeadas ("fast", "reasoning"...) sobre eles, como o formulário de
// Configurações os edita. Puro; espelha `desktop/src/lib/modelPolicies.ts`.

import type { AgentSettings, ModelPolicy } from "./messages";

/** A parte do formulário que estas cascatas leem e mudam. */
export interface PolicyForm {
  agents: AgentSettings[];
  modelPolicies: ModelPolicy[];
}

/** Todo id a que um agente pode ser limitado: os provedores, depois os combos, depois as políticas. */
export function delegationCandidates(providers: { id: string }[], combos: { id: string }[], policies: { id: string }[]): string[] {
  return [...providers, ...combos, ...policies].map((m) => m.id).filter((id) => id.trim() !== "");
}

const renameIn = (list: string[] | undefined, from: string, to: string) => list?.map((m) => (m === from ? to : m));

/** Um provedor ou combo foi renomeado: as políticas que ele responde o seguem, e os agentes que o listavam também. */
export function renameModel<T extends PolicyForm>(form: T, from: string, to: string): T {
  if (from === to) return form;
  return {
    ...form,
    modelPolicies: form.modelPolicies.map((p) => (p.model === from ? { ...p, model: to } : p)),
    agents: form.agents.map((a) => (a.delegationModels?.includes(from) ? { ...a, delegationModels: renameIn(a.delegationModels, from, to) } : a)),
  };
}

/** Uma política foi renomeada: os agentes que a listavam a seguem. */
export function renamePolicy<T extends PolicyForm>(form: T, from: string, to: string): T {
  if (from === to) return form;
  return { ...form, agents: form.agents.map((a) => (a.delegationModels?.includes(from) ? { ...a, delegationModels: renameIn(a.delegationModels, from, to) } : a)) };
}

/** Um provedor ou combo vai embora: as políticas que ele respondia vão junto, e a lista de todo agente o esquece, e a elas. Uma lista que
 * fica vazia deixa a escolha aberta de novo. */
export function dropModel<T extends PolicyForm>(form: T, id: string): T {
  const gone = new Set([id, ...form.modelPolicies.filter((p) => p.model === id).map((p) => p.id)]);
  return {
    ...form,
    modelPolicies: form.modelPolicies.filter((p) => p.model !== id),
    agents: form.agents.map((a) => (a.delegationModels?.some((m) => gone.has(m)) ? { ...a, delegationModels: a.delegationModels.filter((m) => !gone.has(m)) } : a)),
  };
}

/** Uma política vai embora: os agentes que a listavam a esquecem. */
export function dropPolicy<T extends PolicyForm>(form: T, id: string): T {
  return {
    ...form,
    modelPolicies: form.modelPolicies.filter((p) => p.id !== id),
    agents: form.agents.map((a) => (a.delegationModels?.includes(id) ? { ...a, delegationModels: a.delegationModels.filter((m) => m !== id) } : a)),
  };
}

/** O nome com que uma política nova começa: `policy-1`, `policy-2`... livre entre provedores, combos e políticas. */
export function nextPolicyId(taken: string[]): string {
  let n = 1;
  while (taken.includes(`policy-${n}`)) n += 1;
  return `policy-${n}`;
}

/** O que a lista diz sobre os modelos de um agente, numa frase que o cartão mostra embaixo. */
export function delegationSummary(models: string[] | undefined): string {
  if (!models || models.length === 0) return "Aberto: o agente escolhe qualquer modelo para cada tarefa que delega.";
  if (models.length === 1) return `Ditado: toda tarefa que ele delega roda em ${models[0]}.`;
  return `Limitado a ${models.length} modelos; ${models[0]} é o que uma tarefa recebe quando o agente não escolhe.`;
}
