/** P122 — os tipos de ação que um agente pode ser obrigado a ter aprovados mesmo quando age sozinho. Os ids são os de
 * `warden_core::autonomy::Category`, como o config, o hub e `approvalRequired` os escrevem. */
export interface ApprovalCategory {
  id: string;
  label: string;
}

export const APPROVAL_CATEGORIES: ApprovalCategory[] = [
  { id: "delete_data", label: "Apagar dados" },
  { id: "spend_money", label: "Gastar dinheiro" },
  { id: "critical_infra", label: "Mexer em infraestrutura crítica (shell, SSH, nós)" },
  { id: "external_message", label: "Enviar mensagem externa (navegador, serviços de fora)" },
  { id: "publish_code", label: "Publicar código" },
  { id: "important_config", label: "Alterar configuração importante (tarefas agendadas)" },
  { id: "elevated_agent", label: "Criar ou alterar agentes" },
];

/** O nome de uma categoria, ou o próprio id quando um hub mais novo manda uma que esta tela não conhece. */
export function approvalCategoryLabel(id: string): string {
  return APPROVAL_CATEGORIES.find((c) => c.id === id)?.label ?? id;
}
