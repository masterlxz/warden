import { useState } from "react";
import AgentTasksView from "./AgentTasksView";
import OrgView from "./OrgView";

/** P120, P123 — a organização dos agentes e o trabalho que eles passam uns aos outros, lado a lado. Um nó da árvore abre as tarefas do agente
 * (o filtro daqui) ou uma conversa nova com ele (quem monta esta tela decide como, porque o chat é de outra aba). */
export default function AgentsView({ onOpenChat }: { onOpenChat: (id: string) => void }) {
  const [part, setPart] = useState<"org" | "tasks">("org");
  const [taskAgent, setTaskAgent] = useState<string | null>(null);

  return (
    <div className="agents-view">
      <nav className="tab-bar tab-bar--sub">
        <button type="button" className={part === "org" ? "tab tab--active" : "tab"} onClick={() => setPart("org")}>
          Organização
        </button>
        <button
          type="button"
          className={part === "tasks" ? "tab tab--active" : "tab"}
          onClick={() => {
            setTaskAgent(null);
            setPart("tasks");
          }}
        >
          Tarefas
        </button>
      </nav>
      {part === "org" ? (
        <OrgView
          onOpenChat={onOpenChat}
          onOpenTasks={(id) => {
            setTaskAgent(id);
            setPart("tasks");
          }}
        />
      ) : (
        <AgentTasksView agent={taskAgent} onClearAgent={() => setTaskAgent(null)} />
      )}
    </div>
  );
}
