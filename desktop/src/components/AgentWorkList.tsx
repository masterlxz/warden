import { workLabel, type AgentWorkItem } from "../lib/agentWork";

interface AgentWorkListProps {
  agent: string;
  items: AgentWorkItem[];
  /** Conversations with a turn waiting for its answer. */
  answeringIds: string[];
  onOpen: (conversationId: string) => void;
  onBack: () => void;
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
function AgentWorkList({ agent, items, answeringIds, onOpen, onBack }: AgentWorkListProps) {
  return (
    <div className="chat-area">
      <div className="chat-header">
        <button type="button" className="chat-header-action" onClick={onBack}>
          ← Channel
        </button>
        <span className="chat-header-label">{agent}: notes and runs</span>
      </div>
      {items.length === 0 ? (
        <p className="agent-work-empty">{agent} has not traded notes with other agents or run on its own yet.</p>
      ) : (
        <ul className="agent-work-list" aria-label={`Notes and runs of ${agent}`}>
          {items.map((item) => (
            <li key={item.id}>
              <button type="button" className="agent-work-item" onClick={() => onOpen(item.id)}>
                <span className="agent-work-title">
                  {item.title}
                  <span className="agent-work-kind"> · {workLabel(item)}</span>
                </span>
                <span className="agent-work-meta">{answeringIds.includes(item.id) ? "answering…" : shortDate(item.updatedAt)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export default AgentWorkList;
