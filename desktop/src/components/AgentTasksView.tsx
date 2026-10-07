import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentTask, AgentTaskAction } from "../types";
import { hubAgentTasks, hubControlAgentTask } from "../lib/hub";
import { ACTION_LABEL, actionsFor, durationLabel, formatTokens, groupTasks, involvingAgent, STATE_LABEL, STATE_MARK, type TaskGroup } from "../lib/agentTasks";
import { KeyCancelled, usePairingKey } from "./PairingKeyDialog";

const REFRESH_MS = 3000;
const dateFormatter = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" });

function GroupHeader({ group }: { group: TaskGroup }) {
  const { counts } = group;
  const parts = [
    counts.done > 0 && `${counts.done} done`,
    counts.running > 0 && `${counts.running} running`,
    counts.waiting > 0 && `${counts.waiting} waiting for an agent`,
    counts.paused > 0 && `${counts.paused} paused`,
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

function TaskItem({ task, depth, now, busy, onAction }: { task: AgentTask; depth: number; now: number; busy: boolean; onAction: (task: AgentTask, action: AgentTaskAction) => void }) {
  const [open, setOpen] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);
  const detail = task.state === "done" ? task.result : task.error;
  const duration = durationLabel(task, now);
  const actions = actionsFor(task);
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
      {actions.length > 0 && (
        <div className="agent-work-actions">
          {actions.map((action) =>
            action === "cancel" ? (
              confirmStop ? (
                <span key={action} className="agent-work-actions">
                  <button type="button" className="provider-delete-btn" disabled={busy} onClick={() => { setConfirmStop(false); onAction(task, action); }}>
                    Stop it{task.parentId ? "" : " and its subtasks"}
                  </button>
                  <button type="button" className="settings-browse-btn" onClick={() => setConfirmStop(false)}>
                    Keep it
                  </button>
                </span>
              ) : (
                <button key={action} type="button" className="settings-browse-btn" disabled={busy} onClick={() => setConfirmStop(true)}>
                  {ACTION_LABEL[action]}
                </button>
              )
            ) : (
              <button key={action} type="button" className="settings-browse-btn" disabled={busy} onClick={() => onAction(task, action)}>
                {ACTION_LABEL[action]}
              </button>
            ),
          )}
        </div>
      )}
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
 * and what it cost. It refreshes while it is open. For this computer it reads the log the engine writes, and for a hub in use it
 * asks the hub. A task still running there can be paused, resumed or stopped (with its subtasks); on a hub that asks for the
 * pairing key, like any change to it. */
function AgentTasksView({ remote = false, agent = null, onClearAgent }: { remote?: boolean; agent?: string | null; onClearAgent?: () => void }) {
  const [tasks, setTasks] = useState<AgentTask[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [busy, setBusy] = useState<string | null>(null);
  const { askKey, dialog } = usePairingKey();

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

  const control = useCallback(
    async (task: AgentTask, action: AgentTaskAction) => {
      setBusy(task.id);
      try {
        if (remote) {
          setTasks(await hubControlAgentTask(askKey, task.id, action));
        } else {
          await invoke("control_agent_task", { taskId: task.id, action });
          await load();
        }
        setError(null);
      } catch (err) {
        if (!(err instanceof KeyCancelled)) setError(String(err));
      } finally {
        setBusy(null);
      }
    },
    [remote, askKey, load],
  );

  const groups = useMemo(() => groupTasks(agent ? involvingAgent(tasks ?? [], agent) : (tasks ?? [])), [tasks, agent]);

  return (
    <div className="settings-view">
      <h2 className="settings-title">Agent work</h2>
      <p className="settings-hint">
        Tasks that agents handed to each other in the background, {remote ? "on the hub" : "on this computer"}, with where each one is and what
        it cost. A manager picks the agent, and the model, of each task.
      </p>
      {agent && (
        <p className="settings-hint">
          Only the tasks of <strong>{agent}</strong>: the ones it was given and the ones it delegated.{" "}
          {onClearAgent && (
            <button type="button" className="settings-browse-btn" onClick={onClearAgent}>
              Show everyone
            </button>
          )}
        </p>
      )}
      {error && <p className="usage-error">{error}</p>}
      {tasks === null && !error && <p className="settings-hint">Loading…</p>}
      {tasks !== null && groups.length === 0 && (
        <p className="settings-hint">{agent ? `No task involves ${agent} yet.` : 'Nothing yet. When an agent delegates a task with "background", it shows up here.'}</p>
      )}
      {groups.map((group) => (
        <section key={group.group} className="agent-work-group">
          <GroupHeader group={group} />
          <ul className="agent-work-list">
            {group.rows.map(({ task, depth }) => (
              <TaskItem key={task.id} task={task} depth={depth} now={now} busy={busy === task.id} onAction={(t, action) => void control(t, action)} />
            ))}
          </ul>
        </section>
      ))}
      {dialog}
    </div>
  );
}

export default AgentTasksView;
