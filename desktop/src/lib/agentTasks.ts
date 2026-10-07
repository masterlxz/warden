// P123 — the work agents delegate to each other, as the "Agent work" screen shows it: the tasks one turn started are a group,
// with a progress counted over them. Pure, so it can be tested without a window.

import type { AgentTask, AgentTaskAction, AgentTaskState } from "../types";

/** A task as the list shows it: how many levels under the turn's own agent it sits (0 for the tasks the agent started itself). */
export interface TaskRow {
  task: AgentTask;
  depth: number;
}

export interface TaskGroup {
  group: string;
  /** The agent that delegated, if there was one. */
  owner: string | null;
  /** Oldest first, in the order the turn started them. */
  tasks: AgentTask[];
  /** The same tasks as a tree flattened for display: each one followed by its subtasks. */
  rows: TaskRow[];
  total: number;
  /** Done, failed or cancelled: no longer waiting for anything. */
  finished: number;
  counts: Record<AgentTaskState, number>;
  /** Whole percent of the tasks that are finished. */
  percent: number;
  /** Tokens the tasks that reported any used. */
  totalTokens: number;
  /** Something in the group is still pending, running, waiting for another agent or paused. */
  active: boolean;
  createdAtMs: number;
}

export const STATE_LABEL: Record<AgentTaskState, string> = {
  pending: "Pending",
  running: "Running",
  waiting: "Waiting for agent",
  paused: "Paused",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

/** The mark shown next to a state. */
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

/** What a person can do to this task right now: only a task running in the process that answered can be controlled. A task that
 * hasn't started can only be stopped, and so can a delegation the agent is waiting on; a paused one can be resumed. */
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

export const ACTION_LABEL: Record<AgentTaskAction, string> = { pause: "Pause", resume: "Resume", cancel: "Stop" };

/** The tasks of one group as a tree: each task followed by its subtasks, the subtasks one level deeper. A task whose parent isn't in the
 * group (or a loop that shouldn't exist) is shown at the top rather than lost. */
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

/** The tasks that involve one agent: the ones it was given to do and the ones it delegated (P120, from a node of the tree). The tasks of
 * the subtasks it started keep their place under it because they are the ones it delegated. */
export function involvingAgent(tasks: AgentTask[], agent: string): AgentTask[] {
  return tasks.filter((t) => t.assignee === agent || t.owner === agent);
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
