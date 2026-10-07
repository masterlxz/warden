import type { ConversationSummary } from "../hub/messages";

interface Props {
  /** The hub's configured agents. */
  agentIds: string[];
  /** The id of each agent's channel, by agent (the hub makes it); an agent not here yet has not been asked for. */
  channels: Record<string, string>;
  conversations: ConversationSummary[];
  /** The agent whose channel is open. */
  activeAgent: string;
  /** Conversations with a turn waiting for its answer. */
  pendingIds: string[];
  /** The connection is down: reading is fine, opening a channel is not. */
  disabled: boolean;
  onOpen: (agentId: string) => void;
}

/** "14:05" today, "12/09" before that. */
function shortDate(millis: number): string {
  const date = new Date(millis);
  return date.toDateString() === new Date().toDateString()
    ? date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString(undefined, { day: "2-digit", month: "2-digit" });
}

/** P121 — the agents as contacts: each has one conversation with the person, its channel, which stays where it was left. Newest first, the
 * ones never spoken to at the end. Same look as the list of conversations (and the same drawer on phones). */
export default function AgentContacts({ agentIds, channels, conversations, activeAgent, pendingIds, disabled, onOpen }: Props) {
  const rows = agentIds
    .map((id) => {
      const channelId = channels[id];
      const channel = channelId ? conversations.find((c) => c.id === channelId) : undefined;
      return { id, channel, answering: channelId !== undefined && pendingIds.includes(channelId) };
    })
    .sort((a, b) => (b.channel?.updatedAt ?? 0) - (a.channel?.updatedAt ?? 0));

  return (
    <aside className="conversations" aria-label="Agentes">
      {rows.length === 0 ? (
        <p className="conversations-empty">Nenhum agente configurado ainda.</p>
      ) : (
        <ul className="conversations-list">
          {rows.map(({ id, channel, answering }) => {
            const active = id === activeAgent;
            return (
              <li key={id} className={active ? "conversation conversation--active" : "conversation"}>
                <button type="button" className="conversation-open" disabled={disabled} onClick={() => onOpen(id)} aria-current={active ? "true" : undefined}>
                  <span className="conversation-title">{id}</span>
                  <span className="conversation-meta">{answering ? "respondendo…" : channel ? shortDate(channel.updatedAt) : "nova"}</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
}
