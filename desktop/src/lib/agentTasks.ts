// P123 — the work agents delegate to each other, as the "Agent work" screen shows it: the tasks one turn started are a group,
// with a progress counted over them. Pure, so it can be tested without a window.

import type { AgentTask, AgentTaskState } from "../types";

export interface TaskGroup {
  group: string;
  /** The agent that delegated, if there was one. */
  owner: string | null;
  /** Oldest first, in the order the turn started them. */
  tasks: AgentTask[];
  total: number;
  /** Done, failed or cancelled: no longer waiting for anything. */
  finished: number;
  counts: Record<AgentTaskState, number>;
  /** Whole percent of the tasks that are finished. */
  percent: number;
  /** Tokens the tasks that reported any used. */
  totalTokens: number;
  /** Something in the group is still pending or running. */
  active: boolean;
  createdAtMs: number;
}

export const STATE_LABEL: Record<AgentTaskState, string> = {
  pending: "Pending",
  running: "Running",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

/** The mark shown next to a state. */
export const STATE_MARK: Record<AgentTaskState, string> = {
  pending: "○",
  running: "◐",
  done: "✓",
  failed: "⚠",
  cancelled: "⏹",
};

const emptyCounts = (): Record<AgentTaskState, number> => ({ pending: 0, running: 0, done: 0, failed: 0, cancelled: 0 });

/** The tasks grouped by the turn that started them, the newest group first. A state this app doesn't know counts as pending. */
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

/** `1,284` up to 999, `12.9K` beyond. */
export function formatTokens(n: number): string {
  return new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 }).format(n);
}

/** How long a task ran (or has been running), in a short form: `4s`, `2m 05s`, `1h 03m`. `null` before it started. */
export function durationLabel(task: AgentTask, now: number): string | null {
  if (task.startedAtMs == null) return null;
  const seconds = Math.max(0, Math.round(((task.finishedAtMs ?? now) - task.startedAtMs) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}
