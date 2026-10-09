import { workLabel, type AgentWorkItem } from "./lib/agentWork";

interface Props {
  agent: string;
  items: AgentWorkItem[];
  /** Conversations with a turn waiting for its answer. */
  pendingIds: string[];
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

/** P121 — o que um agente fez fora do canal: os recados que trocou com outros agentes e as execuções sem ninguém olhando. Cada um abre no
 * chat como a conversa que é. */
export default function AgentWorkList({ agent, items, pendingIds, onOpen, onBack }: Props) {
  return (
    <div className="channels-view">
      <header className="thread-bar">
        <button type="button" className="link-button" onClick={onBack}>
          ← Canais
        </button>
        <strong>{agent}: recados e execuções</strong>
      </header>
      {items.length === 0 ? (
        <p className="hub-empty">{agent} ainda não trocou recados com outros agentes nem rodou sozinho.</p>
      ) : (
        <ul className="channel-list" aria-label={`Recados e execuções de ${agent}`}>
          {items.map((item) => (
            <li key={item.id}>
              <button type="button" className="channel-row" onClick={() => onOpen(item.id)}>
                <span className="channel-name">
                  {item.title}
                  <span className="channel-meta"> · {workLabel(item)}</span>
                </span>
                <span className="channel-meta">{pendingIds.includes(item.id) ? "respondendo…" : shortDate(item.updatedAt)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
