// P123 — o que o limite de modelos de um agente diz, numa frase. Espelha `web/src/hub/modelPolicies.ts` (a extensão só mostra os
// limites e as políticas; quem os edita é o desktop ou a web).

/** O que a lista diz sobre os modelos de um agente, numa frase que o cartão mostra embaixo. */
export function delegationSummary(models: string[] | undefined): string {
  if (!models || models.length === 0) return "Aberto: o agente escolhe qualquer modelo para cada tarefa que delega.";
  if (models.length === 1) return `Ditado: toda tarefa que ele delega roda em ${models[0]}.`;
  return `Limitado a ${models.length} modelos; ${models[0]} é o que uma tarefa recebe quando o agente não escolhe.`;
}
