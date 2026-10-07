import { useCallback, useEffect, useMemo, useState } from "react";
import { TaskError, type ServerConnection } from "../hub/connection";
import type { AgentTask, AgentTaskAction } from "../hub/messages";
import { ACTION_LABEL, actionsFor, durationLabel, formatTokens, groupTasks, involvingAgent, STATE_LABEL, STATE_MARK, type TaskGroup } from "../hub/agentTasks";

const REFRESH_MS = 3000;
const dateFormatter = new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" });

function GroupHeader({ group }: { group: TaskGroup }) {
  const { counts } = group;
  const parts = [
    counts.done > 0 && `${counts.done} concluída${counts.done > 1 ? "s" : ""}`,
    counts.running > 0 && `${counts.running} em andamento`,
    counts.waiting > 0 && `${counts.waiting} aguardando agente`,
    counts.paused > 0 && `${counts.paused} pausada${counts.paused > 1 ? "s" : ""}`,
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

function TaskItem({ task, depth, now, onAsk }: { task: AgentTask; depth: number; now: number; onAsk: (task: AgentTask, action: AgentTaskAction) => void }) {
  const [open, setOpen] = useState(false);
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
          {actions.map((action) => (
            <button key={action} type="button" className="link-button" onClick={() => onAsk(task, action)}>
              {ACTION_LABEL[action]}
            </button>
          ))}
        </div>
      )}
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
 * gerente e quanto custou. Atualiza enquanto a aba está aberta. Uma tarefa que ainda roda no hub pode ser pausada, retomada ou
 * parada (com as subtarefas), e isso pede a chave de pareamento, como toda mudança no hub. */
export default function AgentTasksView({ conn, agent = null, onClearAgent }: { conn: ServerConnection | null; agent?: string | null; onClearAgent?: () => void }) {
  const [tasks, setTasks] = useState<AgentTask[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [asking, setAsking] = useState<{ task: AgentTask; action: AgentTaskAction } | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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

  function cancelKey() {
    setAsking(null);
    setPairingKey("");
    setKeyError(null);
  }

  async function confirm() {
    if (!conn || !asking) return;
    setBusy(true);
    setKeyError(null);
    try {
      setTasks(await conn.controlAgentTask(pairingKey, asking.task.id, asking.action));
      setError(null);
      cancelKey();
    } catch (err) {
      if (err instanceof TaskError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        cancelKey();
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setBusy(false);
    }
  }

  const keyPrompt = asking && (
    <form
      className="settings-confirm devices-confirm"
      onSubmit={(e) => {
        e.preventDefault();
        void confirm();
      }}
    >
      <p>
        {ACTION_LABEL[asking.action]} a tarefa de <strong>{asking.task.assignee}</strong>
        {asking.action === "cancel" ? " (e as subtarefas dela)" : ""}.
      </p>
      <label className="settings-field">
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        <span className="field-hint">A mesma do primeiro login. É pedida a cada mudança.</span>
      </label>
      {keyError && <p className="error-banner">{keyError}</p>}
      <div className="skills-actions">
        <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : "Confirmar"}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancelKey}>
          Cancelar
        </button>
      </div>
    </form>
  );

  const groups = useMemo(() => groupTasks(agent ? involvingAgent(tasks ?? [], agent) : (tasks ?? [])), [tasks, agent]);

  return (
    <div className="usage-view">
      <p className="skills-hint">
        Tarefas que os agentes passaram uns aos outros em segundo plano neste hub, com onde cada uma está e quanto custou. O gerente escolhe o
        agente, e o modelo, de cada tarefa.
      </p>
      {agent && (
        <p className="skills-hint">
          Só as tarefas de <strong>{agent}</strong>: as que ele recebeu e as que ele delegou.{" "}
          {onClearAgent && (
            <button type="button" className="link-button" onClick={onClearAgent}>
              Mostrar todos
            </button>
          )}
        </p>
      )}
      {error && <p className="error-banner">{error}</p>}
      {keyPrompt}
      {tasks === null && !error && <p className="skills-hint">Carregando…</p>}
      {tasks !== null && groups.length === 0 && <p className="skills-hint">{agent ? `Nenhuma tarefa envolve ${agent} ainda.` : 'Nada ainda. Quando um agente delega uma tarefa com "background", ela aparece aqui.'}</p>}
      {groups.map((group) => (
        <section key={group.group} className="agent-work-group">
          <GroupHeader group={group} />
          <ul className="agent-work-list">
            {group.rows.map(({ task, depth }) => (
              <TaskItem key={task.id} task={task} depth={depth} now={now} onAsk={(t, action) => { setError(null); setKeyError(null); setAsking({ task: t, action }); }} />
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
