import { useCallback, useEffect, useMemo, useState } from "react";
import type { ServerConnection } from "../hub/connection";
import type { AgentTask } from "../hub/messages";
import { durationLabel, formatTokens, groupTasks, STATE_LABEL, STATE_MARK, type TaskGroup } from "../hub/agentTasks";

const REFRESH_MS = 3000;
const dateFormatter = new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" });

function GroupHeader({ group }: { group: TaskGroup }) {
  const { counts } = group;
  const parts = [
    counts.done > 0 && `${counts.done} concluída${counts.done > 1 ? "s" : ""}`,
    counts.running > 0 && `${counts.running} em andamento`,
    counts.waiting > 0 && `${counts.waiting} aguardando agente`,
    counts.pending > 0 && `${counts.pending} pendente${counts.pending > 1 ? "s" : ""}`,
    counts.failed > 0 && `${counts.failed} falhou`,
    counts.cancelled > 0 && `${counts.cancelled} cancelada${counts.cancelled > 1 ? "s" : ""}`,
  ].filter(Boolean);
  return (
    <div className="agent-work-header">
      <div className="agent-work-title">
        <strong>{group.owner ?? "Um agente"}</strong>
        <span className="skills-hint">{dateFormatter.format(group.createdAtMs)}</span>
        {group.active && <span className="agent-work-live">trabalhando</span>}
      </div>
      <div
        className="agent-work-bar"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={group.percent}
        aria-label={`${group.finished} de ${group.total} tarefas terminadas`}
      >
        <div className="agent-work-bar-fill" style={{ width: `${group.percent}%` }} />
      </div>
      <div className="skills-hint">
        {group.finished} de {group.total} terminadas ({group.percent}%) · {parts.join(", ")}
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
          <p className="skills-hint">Tarefa</p>
          <pre className="approval-detail">{task.objective}</pre>
          {detail ? (
            <>
              <p className="skills-hint">{task.state === "done" ? "Resultado" : "Por que parou"}</p>
              <pre className="approval-detail">{detail}</pre>
            </>
          ) : (
            <p className="skills-hint">{STATE_LABEL[task.state]}: ainda sem resultado.</p>
          )}
        </div>
      )}
    </li>
  );
}

/** P123 — o trabalho que os agentes passaram uns aos outros em segundo plano: quem faz o quê, até onde foi o lote de cada
 * gerente e quanto custou. Só leitura; atualiza enquanto a aba está aberta. */
export default function AgentTasksView({ conn }: { conn: ServerConnection | null }) {
  const [tasks, setTasks] = useState<AgentTask[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const load = useCallback(async () => {
    if (!conn) return;
    try {
      setTasks(await conn.listAgentTasks());
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
    setNow(Date.now());
  }, [conn]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const groups = useMemo(() => groupTasks(tasks ?? []), [tasks]);

  return (
    <div className="usage-view">
      <p className="skills-hint">
        Tarefas que os agentes passaram uns aos outros em segundo plano neste hub, com onde cada uma está e quanto custou. O gerente escolhe o
        agente, e o modelo, de cada tarefa.
      </p>
      {error && <p className="error-banner">{error}</p>}
      {tasks === null && !error && <p className="skills-hint">Carregando…</p>}
      {tasks !== null && groups.length === 0 && <p className="skills-hint">Nada ainda. Quando um agente delega uma tarefa com "background", ela aparece aqui.</p>}
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
