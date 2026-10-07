import { useState } from "react";
import type { ConversationSummary } from "../protocol/messages";
import type { OkResponse } from "../background/popup_protocol";

interface Props {
  agentIds: string[];
  /** The id of each agent's channel (the hub makes it); an agent not here yet has not been answered for. */
  channels: Record<string, string>;
  conversations: ConversationSummary[];
  pendingIds: string[];
  /** The channel is open in the chat: the panel goes to it. */
  onOpened: () => void;
}

/** "14:05" today, "12/09" before that. */
function shortDate(millis: number): string {
  const date = new Date(millis);
  return date.toDateString() === new Date().toDateString()
    ? date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString(undefined, { day: "2-digit", month: "2-digit" });
}

/** P121 — the agents as contacts: each has one conversation with the person, its channel. Newest first, the ones never spoken to at the end.
 * Opening one shows it in the chat tab, with the agent fixed. */
export default function ChannelsView({ agentIds, channels, conversations, pendingIds, onOpened }: Props) {
  const [error, setError] = useState<string | null>(null);

  const rows = agentIds
    .map((id) => {
      const channelId = channels[id];
      const channel = channelId ? conversations.find((c) => c.id === channelId) : undefined;
      return { id, channel, answering: channelId !== undefined && pendingIds.includes(channelId) };
    })
    .sort((a, b) => (b.channel?.updatedAt ?? 0) - (a.channel?.updatedAt ?? 0));

  async function open(agentId: string) {
    setError(null);
    const res = (await chrome.runtime.sendMessage({ type: "openAgentChannel", agentId })) as OkResponse;
    if (res.ok) onOpened();
    else setError(res.error ?? "falhou");
  }

  return (
    <div className="channels-view">
      {error && <p className="error-banner">{error}</p>}
      {rows.length === 0 ? (
        <p className="hub-empty">Nenhum agente configurado ainda.</p>
      ) : (
        <ul className="channel-list">
          {rows.map(({ id, channel, answering }) => (
            <li key={id}>
              <button type="button" className="channel-row" onClick={() => void open(id)}>
                <span className="channel-name">{id}</span>
                <span className="channel-meta">{answering ? "respondendo…" : channel ? shortDate(channel.updatedAt) : "nova"}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
