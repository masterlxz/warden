// P123 — o trabalho que os agentes passam uns aos outros, como a aba "Trabalho dos agentes" mostra: as tarefas que um turno
// começou formam um grupo, com um progresso contado sobre elas. Espelha `desktop/src/lib/agentTasks.ts`.

import type { AgentTask, AgentTaskState } from "./messages";

export interface TaskGroup {
  group: string;
  /** O agente que delegou, se houve. */
  owner: string | null;
  /** Da mais antiga à mais nova, na ordem em que o turno as começou. */
  tasks: AgentTask[];
  total: number;
  /** Concluídas, falhas ou canceladas: nada mais a esperar. */
  finished: number;
  counts: Record<AgentTaskState, number>;
  /** Percentual inteiro das tarefas que terminaram. */
  percent: number;
  /** Tokens usados pelas tarefas que informaram. */
  totalTokens: number;
  /** Algo no grupo ainda está pendente ou rodando. */
  active: boolean;
  createdAtMs: number;
}

export const STATE_LABEL: Record<AgentTaskState, string> = {
  pending: "Pendente",
  running: "Em andamento",
  done: "Concluída",
  failed: "Falhou",
  cancelled: "Cancelada",
};

export const STATE_MARK: Record<AgentTaskState, string> = {
  pending: "○",
  running: "◐",
  done: "✓",
  failed: "⚠",
  cancelled: "⏹",
};

const emptyCounts = (): Record<AgentTaskState, number> => ({ pending: 0, running: 0, done: 0, failed: 0, cancelled: 0 });

/** As tarefas agrupadas pelo turno que as começou, o grupo mais novo primeiro. Um estado desconhecido conta como pendente. */
export function groupTasks(tasks: AgentTask[]): TaskGroup[] {
  const byGroup = new Map<string, AgentTask[]>();
  for (const task of tasks) {
    const list = byGroup.get(task.group);
    if (list) list.push(task);
    else byGroup.set(task.group, [task]);
  }
  const groups: TaskGroup[] = [];
  for (const [group, members] of byGroup) {
    const ordered = [...members].sort((a, b) => a.createdAtMs - b.createdAtMs);
    const counts = emptyCounts();
    for (const task of ordered) counts[task.state in counts ? task.state : "pending"] += 1;
    const finished = counts.done + counts.failed + counts.cancelled;
    groups.push({
      group,
      owner: ordered.find((t) => t.owner)?.owner ?? null,
      tasks: ordered,
      total: ordered.length,
      finished,
      counts,
      percent: Math.round((finished / ordered.length) * 100),
      totalTokens: ordered.reduce((sum, t) => sum + (t.totalTokens ?? 0), 0),
      active: counts.pending + counts.running > 0,
      createdAtMs: ordered[0].createdAtMs,
    });
  }
  return groups.sort((a, b) => b.createdAtMs - a.createdAtMs);
}

export function formatTokens(n: number): string {
  return new Intl.NumberFormat("pt-BR", { notation: "compact", maximumFractionDigits: 1 }).format(n);
}

/** Quanto tempo uma tarefa levou (ou está levando): `4s`, `2m 05s`, `1h 03m`. `null` antes de começar. */
export function durationLabel(task: AgentTask, now: number): string | null {
  if (task.startedAtMs == null) return null;
  const seconds = Math.max(0, Math.round(((task.finishedAtMs ?? now) - task.startedAtMs) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}
