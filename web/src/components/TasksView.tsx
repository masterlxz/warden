import { useCallback, useEffect, useState } from "react";
import { TaskError, type ServerConnection } from "../hub/connection";
import type { Task, TaskInfo } from "../hub/messages";

// Scheduled tasks (P92): a prompt an agent runs on its own, on the hub. The same list, create, edit,
// pause, remove and "run now" as `warden-server tasks` and the desktop's Tasks screen. Every change
// asks for the pairing key, like the API keys: a task runs an agent alone, with its tools and spend.
// Each run lands in the task's conversation (`task-<id>`), which "Abrir conversa" opens in the chat.

type ScheduleKind = "every" | "cron" | "once";

interface EditorState {
  /** The id being edited; absent for a new task. */
  originalId?: string;
  task: Task;
  kind: ScheduleKind;
  value: string;
}

type Change =
  | { kind: "save"; editor: EditorState }
  | { kind: "toggle"; task: TaskInfo }
  | { kind: "delete"; task: TaskInfo }
  | { kind: "run"; task: TaskInfo };

const dateFormatter = new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" });
const browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone;

const placeholders: Record<ScheduleKind, string> = { every: "1d", cron: "0 8 * * 1-5", once: "2026-10-01T09:00" };
const hints: Record<ScheduleKind, string> = {
  every: "Um número e m, h ou d (30m, 2h, 1d), contado da última execução.",
  cron: "Cinco campos: minuto, hora, dia do mês, mês, dia da semana. 0 8 * * 1-5 = 8h nos dias úteis.",
  once: "Data e hora locais, AAAA-MM-DDTHH:MM.",
};

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function scheduleKind(task: Task): ScheduleKind {
  return task.cron ? "cron" : task.once ? "once" : "every";
}

function scheduleLabel(task: Task): string {
  const zone = task.timezone ? ` (${task.timezone})` : "";
  if (task.cron) return `cron ${task.cron}${zone}`;
  if (task.once) return `uma vez em ${task.once}${zone}`;
  return `a cada ${task.every ?? "?"}`;
}

function fromEditor(editor: EditorState): Task {
  const { every: _e, cron: _c, once: _o, ...rest } = editor.task;
  return { ...rest, [editor.kind]: editor.value.trim() };
}

function newEditor(): EditorState {
  return { task: { id: "", prompt: "", enabled: true, timezone: browserZone }, kind: "every", value: "" };
}

function editorFor(task: TaskInfo): EditorState {
  const kind = scheduleKind(task);
  const plain: Task = { id: task.id, agentId: task.agentId, prompt: task.prompt, timezone: task.timezone, enabled: task.enabled };
  return { originalId: task.id, task: plain, kind, value: task[kind] ?? "" };
}

function status(task: TaskInfo): string {
  if (task.scheduleError) return `Agendamento inválido: ${task.scheduleError}`;
  const parts: string[] = [];
  if (task.running) parts.push("rodando agora");
  else if (task.lastRunAtMs) parts.push(`última: ${dateFormatter.format(task.lastRunAtMs)}${task.lastError ? " (falhou)" : ""}`);
  else parts.push("nunca rodou");
  if (!task.enabled) parts.push("pausada");
  else if (task.nextRunAtMs) parts.push(`próxima: ${dateFormatter.format(task.nextRunAtMs)}`);
  else if (!task.running) parts.push("concluída");
  return parts.join(" · ");
}

export default function TasksView({ conn, onOpenConversation }: { conn: ServerConnection | null; onOpenConversation: (id: string) => void }) {
  const [tasks, setTasks] = useState<TaskInfo[] | null>(null);
  const [runsHere, setRunsHere] = useState(true);
  const [agentIds, setAgentIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [asking, setAsking] = useState<Change | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!conn) return;
    try {
      const list = await conn.listTasks();
      setTasks(list.tasks);
      setRunsHere(list.runsHere);
      setError(null);
    } catch (err) {
      setTasks((current) => current ?? []);
      setError(`falha ao listar as tarefas: ${message(err)}`);
    }
  }, [conn]);

  useEffect(() => {
    void refresh();
    if (!conn) return;
    conn
      .requestSettings()
      .then(({ settings }) => setAgentIds(settings.agents.map((a) => a.id)))
      .catch(() => setAgentIds([]));
    // A run finished (scheduled or "run now"): its status changed.
    return conn.onConversationsChanged((id) => {
      if (id.startsWith("task-")) void refresh();
    });
  }, [conn, refresh]);

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
      const list =
        asking.kind === "save"
          ? await conn.saveTask(pairingKey, fromEditor(asking.editor), asking.editor.originalId)
          : asking.kind === "toggle"
            ? await conn.setTaskEnabled(pairingKey, asking.task.id, !asking.task.enabled)
            : asking.kind === "delete"
              ? await conn.deleteTask(pairingKey, asking.task.id)
              : await conn.runTask(pairingKey, asking.task.id);
      setTasks(list.tasks);
      setRunsHere(list.runsHere);
      if (asking.kind === "save") setEditor(null);
      setConfirmDelete(null);
      setError(null);
      cancelKey();
    } catch (err) {
      if (err instanceof TaskError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        cancelKey();
        setError(message(err));
      }
    } finally {
      setBusy(false);
    }
  }

  function ask(change: Change) {
    setError(null);
    setKeyError(null);
    setAsking(change);
  }

  const labels: Record<Change["kind"], string> = { save: "Salvar a tarefa", toggle: "Confirmar", delete: "Apagar", run: "Rodar agora" };
  const keyPrompt = asking && (
    <form
      className="settings-confirm devices-confirm"
      onSubmit={(e) => {
        e.preventDefault();
        void confirm();
      }}
    >
      <label className="settings-field">
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        <span className="field-hint">A mesma do primeiro login. É pedida a cada mudança.</span>
      </label>
      {keyError && <p className="error-banner">{keyError}</p>}
      <div className="skills-actions">
        <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : labels[asking.kind]}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancelKey}>
          Cancelar
        </button>
      </div>
    </form>
  );

  function update(patch: Partial<EditorState>) {
    setEditor((current) => (current ? { ...current, ...patch } : current));
  }

  function updateTask(patch: Partial<Task>) {
    setEditor((current) => (current ? { ...current, task: { ...current.task, ...patch } } : current));
  }

  if (editor) {
    return (
      <form
        className="skills-editor"
        onSubmit={(e) => {
          e.preventDefault();
          ask({ kind: "save", editor });
        }}
      >
        <strong>{editor.originalId ? `Editar ${editor.originalId}` : "Nova tarefa"}</strong>
        <label>
          Nome
          <input value={editor.task.id} onChange={(e) => updateTask({ id: e.target.value })} placeholder="resumo-diario" required />
          <span className="skills-hint">Letras, números, - e _. A conversa da tarefa se chama task-&lt;nome&gt;.</span>
        </label>
        <label>
          Agente
          <select value={editor.task.agentId ?? ""} onChange={(e) => updateTask({ agentId: e.target.value || undefined })}>
            <option value="">(nenhum)</option>
            {agentIds.map((id) => (
              <option key={id} value={id}>
                {id}
              </option>
            ))}
          </select>
        </label>
        <label>
          O que pedir
          <textarea
            value={editor.task.prompt}
            onChange={(e) => updateTask({ prompt: e.target.value })}
            rows={6}
            placeholder="Resuma as notícias de hoje sobre…"
            required
          />
        </label>
        <label>
          Quando
          <select value={editor.kind} onChange={(e) => update({ kind: e.target.value as ScheduleKind, value: "" })}>
            <option value="every">A cada</option>
            <option value="cron">Cron</option>
            <option value="once">Uma vez</option>
          </select>
        </label>
        <label>
          {editor.kind === "every" ? "Intervalo" : editor.kind === "cron" ? "Expressão cron" : "Data e hora"}
          <input value={editor.value} onChange={(e) => update({ value: e.target.value })} placeholder={placeholders[editor.kind]} required />
          <span className="skills-hint">{hints[editor.kind]}</span>
        </label>
        {editor.kind !== "every" && (
          <label>
            Fuso horário
            <input value={editor.task.timezone ?? ""} onChange={(e) => updateTask({ timezone: e.target.value || undefined })} placeholder={browserZone} />
            <span className="skills-hint">Nome IANA, como America/Sao_Paulo. Vazio: o fuso da máquina do hub.</span>
          </label>
        )}
        {error && <p className="error-banner">{error}</p>}
        {asking ? (
          keyPrompt
        ) : (
          <div className="skills-actions">
            <button type="submit" className="primary-button" disabled={!conn}>
              Salvar
            </button>
            <button
              type="button"
              className="link-button"
              onClick={() => {
                setEditor(null);
                setError(null);
              }}
            >
              Cancelar
            </button>
          </div>
        )}
      </form>
    );
  }

  return (
    <div className="skills-view">
      <div className="skills-toolbar">
        <span className="skills-hint">Pedidos que um agente faz sozinho, no horário marcado. Cada execução cai na conversa da tarefa.</span>
        <button
          type="button"
          className="primary-button"
          onClick={() => {
            setError(null);
            setEditor(newEditor());
          }}
        >
          + Nova
        </button>
      </div>
      {!runsHere && (
        <p className="skills-hint">
          Este hub não executa as tarefas no horário (inicie com <code>warden-server serve --run-tasks</code>, ou ligue no desktop). Elas rodam no
          hub que estiver com isso ligado, e "Rodar agora" roda aqui.
        </p>
      )}
      {error && <p className="error-banner">{error}</p>}
      {asking && keyPrompt}
      {tasks === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : tasks.length === 0 ? (
        <p className="skills-hint">Nenhuma tarefa ainda.</p>
      ) : (
        <ul className="skills-list">
          {tasks.map((task) => (
            <li key={task.id} className="skills-item">
              <div className="skills-item-header">
                <span className="skills-item-name">{task.id}</span>
                {confirmDelete === task.id ? (
                  <span className="skills-actions">
                    <button type="button" className="link-button skills-danger" onClick={() => ask({ kind: "delete", task })}>
                      Apagar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(null)}>
                      Manter
                    </button>
                  </span>
                ) : (
                  <span className="skills-actions">
                    <button type="button" className="link-button" onClick={() => onOpenConversation(`task-${task.id}`)}>
                      Abrir conversa
                    </button>
                    <button type="button" className="link-button" disabled={task.running} onClick={() => ask({ kind: "run", task })}>
                      Rodar agora
                    </button>
                    <button type="button" className="link-button" onClick={() => ask({ kind: "toggle", task })}>
                      {task.enabled ? "Pausar" : "Retomar"}
                    </button>
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => {
                        setError(null);
                        setEditor(editorFor(task));
                      }}
                    >
                      Editar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(task.id)}>
                      Apagar
                    </button>
                  </span>
                )}
              </div>
              <p className="skills-item-description">
                {scheduleLabel(task)} · {task.agentId ? `agente ${task.agentId}` : "sem agente"}
              </p>
              <p className="skills-hint">{status(task)}</p>
              {task.lastError && <p className="skills-hint">Erro: {task.lastError}</p>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
