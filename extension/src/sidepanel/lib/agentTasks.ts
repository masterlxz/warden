// P123 — o trabalho que os agentes passam uns aos outros, como a aba "Trabalho dos agentes" mostra: as tarefas que um turno
// começou formam um grupo, com um progresso contado sobre elas. Espelha `desktop/src/lib/agentTasks.ts`.

import type { AgentTask, AgentTaskAction, AgentTaskState } from "../../protocol/messages";

/** Uma tarefa como a lista a mostra: quantos níveis abaixo do agente do turno ela está (0 para as que o próprio agente começou). */
export interface TaskRow {
  task: AgentTask;
  depth: number;
}

export interface TaskGroup {
  group: string;
  /** O agente que delegou, se houve. */
  owner: string | null;
  /** Da mais antiga à mais nova, na ordem em que o turno as começou. */
  tasks: AgentTask[];
  /** As mesmas tarefas como árvore achatada para mostrar: cada uma seguida das suas subtarefas. */
  rows: TaskRow[];
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
  waiting: "Aguardando agente",
  paused: "Pausada",
  done: "Concluída",
  failed: "Falhou",
  cancelled: "Cancelada",
};

export const STATE_MARK: Record<AgentTaskState, string> = {
  pending: "○",
  running: "◐",
  waiting: "◉",
  paused: "⏸",
  done: "✓",
  failed: "⚠",
  cancelled: "⏹",
};

const emptyCounts = (): Record<AgentTaskState, number> => ({ pending: 0, running: 0, waiting: 0, paused: 0, done: 0, failed: 0, cancelled: 0 });

/** O que dá para fazer com esta tarefa agora: só a que roda no processo do hub que respondeu. A que ainda não começou só pode ser parada;
 * a pausada pode ser retomada; uma delegação que o agente espera também só pode ser parada. */
export function actionsFor(task: AgentTask): AgentTaskAction[] {
  if (!task.controllable) return [];
  switch (task.state) {
    case "pending":
      return ["cancel"];
    case "running":
    case "waiting":
      return task.pausable ? ["pause", "cancel"] : ["cancel"];
    case "paused":
      return ["resume", "cancel"];
    default:
      return [];
  }
}

export const ACTION_LABEL: Record<AgentTaskAction, string> = { pause: "Pausar", resume: "Retomar", cancel: "Parar" };

/** As tarefas de um grupo como árvore: cada tarefa seguida das suas subtarefas, um nível abaixo. Uma tarefa cujo pai não está no grupo
 * (ou um laço que não deveria existir) aparece no topo em vez de se perder. */
function treeRows(ordered: AgentTask[]): TaskRow[] {
  const ids = new Set(ordered.map((t) => t.id));
  const rows: TaskRow[] = [];
  const seen = new Set<string>();
  const add = (task: AgentTask, depth: number) => {
    seen.add(task.id);
    rows.push({ task, depth });
    for (const child of ordered) if (child.parentId === task.id && !seen.has(child.id)) add(child, depth + 1);
  };
  for (const task of ordered) if (!task.parentId || !ids.has(task.parentId)) add(task, 0);
  for (const task of ordered) if (!seen.has(task.id)) add(task, 0);
  return rows;
}

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
      rows: treeRows(ordered),
      total: ordered.length,
      finished,
      counts,
      percent: Math.round((finished / ordered.length) * 100),
      totalTokens: ordered.reduce((sum, t) => sum + (t.totalTokens ?? 0), 0),
      active: counts.pending + counts.running + counts.waiting + counts.paused > 0,
      createdAtMs: ordered[0].createdAtMs,
    });
  }
  return groups.sort((a, b) => b.createdAtMs - a.createdAtMs);
}

/** As tarefas que envolvem um agente: as que ele recebeu e as que ele delegou (P120, a partir de um nó da árvore). */
export function involvingAgent(tasks: AgentTask[], agent: string): AgentTask[] {
  return tasks.filter((t) => t.assignee === agent || t.owner === agent);
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
