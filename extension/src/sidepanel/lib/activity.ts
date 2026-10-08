// P121 — o feed de atividade: quem fez o quê entre os agentes, do mais novo ao mais velho. Puro; a tela só desenha o que sai daqui.
// Espelha `web/src/hub/activity.ts`.

import type { ActivityEvent } from "../../protocol/messages";

/** Como a tela chama o agente que atende sem ninguém escolhido (um evento sem `actor`). */
export const NO_AGENT_NAME = "O assistente";

const who = (name: string) => (name.trim() === "" ? NO_AGENT_NAME : name);

/** As execuções de tarefa agendada e de webhook: o alvo delas é o id da tarefa ou do webhook, não um agente. */
const RUN_KINDS = new Set(["scheduled_ran", "scheduled_failed", "webhook_ran", "webhook_failed"]);

/** O agente que o evento atinge, quando o alvo dele é um agente. */
const targetAgent = (event: ActivityEvent) => (RUN_KINDS.has(event.kind) ? undefined : event.target ?? undefined);

/** A frase de um evento, sem o texto dele (o texto é o objetivo, a resposta ou a mensagem). */
export function headline(event: ActivityEvent): string {
  const actor = who(event.actor);
  const target = event.target ? who(event.target) : "";
  switch (event.kind) {
    case "created_agent":
      return `${actor} criou o agente ${target}`;
    case "removed_agent":
      return `${actor} removeu o agente ${target}`;
    case "scheduled_ran":
      return `${actor} rodou a tarefa agendada ${target}`;
    case "scheduled_failed":
      return `A tarefa agendada ${target} falhou`;
    case "webhook_ran":
      return `${actor} atendeu o webhook ${target}`;
    case "webhook_failed":
      return `O webhook ${target} falhou`;
    case "delegated":
      return `${actor} delegou uma tarefa a ${target}`;
    case "started":
      return `${actor} começou a tarefa`;
    case "done":
      return `${actor} concluiu a tarefa`;
    case "failed":
      return `${actor} não conseguiu concluir a tarefa`;
    case "cancelled":
      return `A tarefa de ${actor} foi cancelada`;
    case "note":
      return `${actor} deixou um recado para ${target}`;
    case "reply":
      return `${actor} respondeu a ${target}`;
    case "messaged_user":
      return `${actor} escreveu para você`;
    default:
      return `${actor}: ${event.kind}`;
  }
}

/** Uma marca curta por tipo, para varrer a lista com o olho. */
export function mark(kind: string): string {
  switch (kind) {
    case "delegated":
      return "→";
    case "started":
      return "▶";
    case "done":
      return "✓";
    case "failed":
    case "scheduled_failed":
    case "webhook_failed":
      return "✕";
    case "cancelled":
      return "■";
    case "created_agent":
      return "+";
    case "removed_agent":
      return "−";
    case "scheduled_ran":
      return "⏱";
    case "webhook_ran":
      return "⚡";
    case "note":
    case "reply":
      return "✉";
    case "messaged_user":
      return "●";
    default:
      return "·";
  }
}

/** Os eventos em que o agente aparece, fazendo ou recebendo. */
export function involving(events: ActivityEvent[], agent: string): ActivityEvent[] {
  return events.filter((e) => e.actor === agent || targetAgent(e) === agent);
}

/** Os agentes que aparecem nos eventos, em ordem alfabética, sem o assistente sem nome (nem o id de uma tarefa agendada ou de um webhook). */
export function agentsIn(events: ActivityEvent[]): string[] {
  const names = new Set<string>();
  for (const e of events) {
    if (e.actor.trim() !== "") names.add(e.actor);
    const target = targetAgent(e);
    if (target && target.trim() !== "") names.add(target);
  }
  return [...names].sort((a, b) => a.localeCompare(b));
}

/** O que um clique num evento abre: as tarefas do agente, a conversa entre dois agentes ou o canal do agente. */
export type Destination = { kind: "tasks"; agent: string } | { kind: "conversation"; id: string } | { kind: "channel"; agent: string; id: string };

export function destination(event: ActivityEvent): Destination | null {
  if (event.kind === "messaged_user") return event.conversationId ? { kind: "channel", agent: event.actor, id: event.conversationId } : null;
  if (event.conversationId) return { kind: "conversation", id: event.conversationId };
  if (event.taskId) {
    const agent = event.kind === "delegated" ? event.target : event.actor;
    return agent ? { kind: "tasks", agent } : null;
  }
  return null;
}

export interface DayGroup {
  /** "Hoje", "Ontem" ou a data. */
  label: string;
  events: ActivityEvent[];
}

const dayKey = (ms: number) => {
  const d = new Date(ms);
  return d.getFullYear() * 10000 + (d.getMonth() + 1) * 100 + d.getDate();
};

/** Os eventos (já do mais novo ao mais velho) separados por dia, com o rótulo de cada dia. */
export function groupByDay(events: ActivityEvent[], now: number): DayGroup[] {
  const formatter = new Intl.DateTimeFormat("pt-BR", { day: "2-digit", month: "long", year: "numeric" });
  const today = dayKey(now);
  const yesterday = dayKey(now - 24 * 60 * 60 * 1000);
  const groups: DayGroup[] = [];
  for (const event of events) {
    const key = dayKey(event.atMs);
    const label = key === today ? "Hoje" : key === yesterday ? "Ontem" : formatter.format(event.atMs);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.events.push(event);
    else groups.push({ label, events: [event] });
  }
  return groups;
}
