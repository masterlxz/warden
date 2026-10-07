import type { AgentEntry, Conversation } from "../types";

interface AgentContactsProps {
  agents: AgentEntry[];
  /** The id of each agent's channel, by agent (the hub makes it); an agent not here yet has not been asked for. */
  channels: Record<string, string>;
  conversations: Conversation[];
  /** The agent whose channel is open. */
  activeAgent: string;
  /** The agents with something the person has not seen in their channel. */
  unreadAgents: string[];
  /** Conversations with a turn waiting for its answer. */
  answeringIds: string[];
  /** The hub is not reachable: reading is fine, opening a channel is not. */
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
 * ones never spoken to at the end. */
function AgentContacts({ agents, channels, conversations, activeAgent, unreadAgents, answeringIds, disabled, onOpen }: AgentContactsProps) {
  const rows = agents
    .map((agent) => {
      const channelId = channels[agent.id];
      const channel = channelId ? conversations.find((c) => c.id === channelId) : undefined;
      return { id: agent.id, channel, answering: channelId !== undefined && answeringIds.includes(channelId) };
    })
    .sort((a, b) => (b.channel?.updatedAt ?? 0) - (a.channel?.updatedAt ?? 0));

  return (
    <aside className="agent-contacts" aria-label="Agents">
      {rows.length === 0 ? (
        <p className="conversation-list-empty">No agents configured yet.</p>
      ) : (
        <ul className="agent-contact-list">
          {rows.map(({ id, channel, answering }) => (
            <li key={id}>
              <button
                type="button"
                className={`agent-contact${id === activeAgent ? " agent-contact--active" : ""}`}
                disabled={disabled}
                onClick={() => onOpen(id)}
                aria-current={id === activeAgent ? "true" : undefined}
              >
                <span className="agent-contact-name">
                  {unreadAgents.includes(id) && <span className="unread-dot" role="img" aria-label="new message" />}
                  {id}
                </span>
                <span className="agent-contact-meta">{answering ? "answering…" : channel ? shortDate(channel.updatedAt) : "new"}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}

export default AgentContacts;
