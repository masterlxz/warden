import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentTask } from "../types";
import { hubAgentTasks } from "../lib/hub";
import { durationLabel, formatTokens, groupTasks, STATE_LABEL, STATE_MARK, type TaskGroup } from "../lib/agentTasks";

const REFRESH_MS = 3000;
const dateFormatter = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" });

function GroupHeader({ group }: { group: TaskGroup }) {
  const { counts } = group;
  const parts = [
    counts.done > 0 && `${counts.done} done`,
    counts.running > 0 && `${counts.running} running`,
    counts.waiting > 0 && `${counts.waiting} waiting for an agent`,
    counts.pending > 0 && `${counts.pending} pending`,
    counts.failed > 0 && `${counts.failed} failed`,
    counts.cancelled > 0 && `${counts.cancelled} cancelled`,
  ].filter(Boolean);
  return (
    <div className="agent-work-header">
      <div className="agent-work-title">
        <strong>{group.owner ?? "An agent"}</strong>
        <span className="settings-hint">{dateFormatter.format(group.createdAtMs)}</span>
        {group.active && <span className="agent-work-live">working</span>}
      </div>
      <div
        className="agent-work-bar"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={group.percent}
        aria-label={`${group.finished} of ${group.total} tasks finished`}
      >
        <div className="agent-work-bar-fill" style={{ width: `${group.percent}%` }} />
      </div>
      <div className="settings-hint">
        {group.finished} of {group.total} finished ({group.percent}%) · {parts.join(", ")}
        {group.totalTokens > 0 && ` · ${formatTokens(group.totalTokens)} tokens`}
      </div>
    </div>
  );
}

function TaskItem({ task, depth, now }: { task: AgentTask; depth: number; now: number }) {
  const [open, setOpen] = useState(false);
  const detail = task.state === "done" ? task.result : task.error;
  const duration = durationLabel(task, now);
  return (
    <li className={`agent-work-task agent-work-task--${task.state}`} style={depth > 0 ? { marginLeft: `${depth * 1.4}em` } : undefined}>
      <button type="button" className="agent-work-task-line" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        <span className="agent-work-mark" title={STATE_LABEL[task.state]} aria-label={STATE_LABEL[task.state]}>
          {STATE_MARK[task.state]}
        </span>
        <span className="agent-work-assignee">{task.assignee}</span>
        <span className="agent-work-objective">{task.objective}</span>
        <span className="agent-work-meta">
          {task.model && <span className="org-badge">{task.model}</span>}
          {duration && <span>{duration}</span>}
          {task.totalTokens != null && <span>{formatTokens(task.totalTokens)} tok</span>}
        </span>
      </button>
      {open && (
        <div className="agent-work-detail">
          <p className="settings-hint">Task</p>
          <pre className="approval-detail">{task.objective}</pre>
          {detail ? (
            <>
              <p className="settings-hint">{task.state === "done" ? "Result" : "Why it stopped"}</p>
              <pre className="approval-detail">{detail}</pre>
            </>
          ) : (
            <p className="settings-hint">{STATE_LABEL[task.state]}: no result yet.</p>
          )}
        </div>
      )}
    </li>
  );
}

/** P123 — the work agents delegated to each other in the background: who is doing what, how far each manager's batch is,
 * and what it cost. Read only; it refreshes while it is open. For this computer it reads the log the engine writes, and for
 * a hub in use it asks the hub. */
function AgentTasksView({ remote = false }: { remote?: boolean }) {
  const [tasks, setTasks] = useState<AgentTask[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const load = useCallback(async () => {
    try {
      setTasks(remote ? await hubAgentTasks() : await invoke<AgentTask[]>("list_agent_tasks"));
      setError(null);
    } catch (err) {
      setError(String(err));
    }
    setNow(Date.now());
  }, [remote]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const groups = useMemo(() => groupTasks(tasks ?? []), [tasks]);

  return (
    <div className="settings-view">
      <h2 className="settings-title">Agent work</h2>
      <p className="settings-hint">
        Tasks that agents handed to each other in the background, {remote ? "on the hub" : "on this computer"}, with where each one is and what
        it cost. A manager picks the agent, and the model, of each task.
      </p>
      {error && <p className="usage-error">{error}</p>}
      {tasks === null && !error && <p className="settings-hint">Loading…</p>}
      {tasks !== null && groups.length === 0 && (
        <p className="settings-hint">Nothing yet. When an agent delegates a task with "background", it shows up here.</p>
      )}
      {groups.map((group) => (
        <section key={group.group} className="agent-work-group">
          <GroupHeader group={group} />
          <ul className="agent-work-list">
            {group.rows.map(({ task, depth }) => (
              <TaskItem key={task.id} task={task} depth={depth} now={now} />
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

export default AgentTasksView;
