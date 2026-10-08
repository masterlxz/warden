import { useState } from "react";
import ActivityView from "./ActivityView";
import AgentTasksView from "./AgentTasksView";
import OrgView from "./OrgView";

/** P120, P123, P121 — a organização dos agentes, o trabalho que eles passam uns aos outros e o feed do que aconteceu, lado a lado. Um nó da
 * árvore abre as tarefas do agente (o filtro daqui) ou uma conversa nova com ele (quem monta esta tela decide como, porque o chat é de outra
 * aba). Um evento do feed abre as tarefas do agente, a conversa entre dois agentes ou o canal dele, os dois últimos também pelo chat. */
export default function AgentsView({
  onOpenChat,
  onOpenConversation,
  onOpenChannel,
}: {
  onOpenChat: (id: string) => void;
  onOpenConversation: (conversationId: string) => void;
  onOpenChannel: (agent: string) => void;
}) {
  const [part, setPart] = useState<"org" | "tasks" | "activity">("org");
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
        <button type="button" className={part === "activity" ? "tab tab--active" : "tab"} onClick={() => setPart("activity")}>
          Atividade
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
      ) : part === "tasks" ? (
        <AgentTasksView agent={taskAgent} onClearAgent={() => setTaskAgent(null)} />
      ) : (
        <ActivityView
          onOpen={(to) => {
            if (to.kind === "tasks") {
              setTaskAgent(to.agent);
              setPart("tasks");
            } else if (to.kind === "conversation") {
              onOpenConversation(to.id);
            } else {
              onOpenChannel(to.agent);
            }
          }}
        />
      )}
    </div>
  );
}
