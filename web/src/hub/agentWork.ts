// P121 — o que um agente fez fora do canal dele: os recados que trocou com outros agentes ("A → B", `message_agent`) e as execuções sem
// ninguém olhando (tarefas agendadas e webhooks, `task-*`). Puro, para testar sem janela. Os ids e o título são os do hub (`activity.rs`).

import type { ConversationSummary } from "./messages";

/** O começo do id da conversa entre dois agentes (`message_agent::thread_id`). */
export const NOTE_PREFIX = "agents-";
/** O começo do id da conversa de uma tarefa agendada (`tasks::CONVERSATION_PREFIX`); a de um webhook é `task-hook-`. */
export const RUN_PREFIX = "task-";
const HOOK_PREFIX = "task-hook-";
const ARROW = " → ";

export type AgentWorkKind = "note-out" | "note-in" | "run-task" | "run-hook";

/** Uma conversa do agente fora do canal. `other` é o colega de um recado. */
export interface AgentWorkItem {
  id: string;
  title: string;
  kind: AgentWorkKind;
  other?: string;
  updatedAt: number;
}

/** Os dois lados de uma conversa "A → B", ou `null` quando o título não tem a seta. */
function ends(title: string): [string, string] | null {
  const at = title.indexOf(ARROW);
  if (at < 0) return null;
  return [title.slice(0, at).trim(), title.slice(at + ARROW.length).trim()];
}

/** As conversas de `agent` fora do canal, as mais novas primeiro: os recados que deixou ou recebeu e as execuções que fez. Uma thread (P125)
 * não entra, ela aparece a partir da mensagem de onde saiu. */
export function agentWork(conversations: ConversationSummary[], agent: string): AgentWorkItem[] {
  const items: AgentWorkItem[] = [];
  for (const c of conversations) {
    if (c.parent) continue;
    if (c.id.startsWith(NOTE_PREFIX)) {
      const pair = ends(c.title);
      if (!pair || pair[0] === pair[1]) continue;
      if (pair[0] === agent) items.push({ id: c.id, title: c.title, kind: "note-out", other: pair[1], updatedAt: c.updatedAt });
      else if (pair[1] === agent) items.push({ id: c.id, title: c.title, kind: "note-in", other: pair[0], updatedAt: c.updatedAt });
    } else if (c.id.startsWith(RUN_PREFIX) && c.agentId === agent) {
      items.push({ id: c.id, title: c.title, kind: c.id.startsWith(HOOK_PREFIX) ? "run-hook" : "run-task", updatedAt: c.updatedAt });
    }
  }
  return items.sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
}

/** O que a lista diz de uma conversa: `para ana`, `de ana`, `tarefa agendada`, `webhook`. */
export function workLabel(item: AgentWorkItem): string {
  switch (item.kind) {
    case "note-out":
      return `para ${item.other}`;
    case "note-in":
      return `de ${item.other}`;
    case "run-task":
      return "tarefa agendada";
    case "run-hook":
      return "webhook";
  }
}

/** O que o botão do canal diz: `Recados e execuções (3)`, ou sem o número quando não há nenhuma. */
export function workButtonLabel(count: number): string {
  return count === 0 ? "Recados e execuções" : `Recados e execuções (${count})`;
}
