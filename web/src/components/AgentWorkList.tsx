import { workLabel, type AgentWorkItem } from "../hub/agentWork";

interface Props {
  agent: string;
  items: AgentWorkItem[];
  /** Conversations with a turn waiting for its answer. */
  pendingIds: string[];
  onOpen: (conversationId: string) => void;
}

/** "14:05" today, "12/09" before that. */
function shortDate(millis: number): string {
  const date = new Date(millis);
  return date.toDateString() === new Date().toDateString()
    ? date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString(undefined, { day: "2-digit", month: "2-digit" });
}

/** P121 — what an agent did outside its channel: the notes it traded with other agents and the runs nobody was watching. Opens each one
 * as the conversation it is. */
export default function AgentWorkList({ agent, items, pendingIds, onOpen }: Props) {
  if (items.length === 0) {
    return <p className="skills-hint agents-empty">{agent} ainda não trocou recados com outros agentes nem rodou sozinho.</p>;
  }
  return (
    <ul className="conversations-list work-list" aria-label={`Recados e execuções de ${agent}`}>
      {items.map((item) => (
        <li key={item.id} className="conversation">
          <button type="button" className="conversation-open" onClick={() => onOpen(item.id)}>
            <span className="conversation-title">
              {item.title}
              <span className="work-kind"> · {workLabel(item)}</span>
            </span>
            <span className="conversation-meta">{pendingIds.includes(item.id) ? "respondendo…" : shortDate(item.updatedAt)}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}
