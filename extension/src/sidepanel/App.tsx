import { useEffect, useState } from "react";
import ConnectionForm from "./ConnectionForm";
import ChatView from "./ChatView";
import ConversationBar from "./ConversationBar";
import SkillsView from "./SkillsView";
import AgentsView from "./AgentsView";
import TabsView from "./TabsView";
import ChannelsView from "./ChannelsView";
import AgentWorkList from "./AgentWorkList";
import { agentWork, workButtonLabel } from "./lib/agentWork";
import ApprovalCard from "./ApprovalCard";
import type { ApprovalPrompt } from "../protocol/messages";
import { threadsOf } from "../protocol/threads";
import type { ChatEntry, ConnectionStatus } from "../background/connection";
import type { BackgroundEvent, ConnectionSettings, ConversationState, GetStatusResponse, OkResponse } from "../background/popup_protocol";

export default function App() {
  const [status, setStatus] = useState<ConnectionStatus>({ kind: "disconnected" });
  const [history, setHistory] = useState<ChatEntry[]>([]);
  const [savedSettings, setSavedSettings] = useState<Partial<ConnectionSettings>>({});
  const [connectError, setConnectError] = useState<string | undefined>(undefined);
  const [connecting, setConnecting] = useState(false);
  /** P78 — kept by the background, which also knows what's waiting on an answer. */
  const [conversationState, setConversationState] = useState<ConversationState>({
    conversations: [],
    activeConversationId: null,
    pendingIds: [],
    agentIds: [],
    agentId: null,
    workdir: null,
    threadParent: null,
    channels: {},
    unreadChannels: [],
  });
  /** P87 — approvals the hub is waiting on, kept by the background. */
  const [approvals, setApprovals] = useState<ApprovalPrompt[]>([]);
  const [tab, setTab] = useState<"chat" | "channels" | "skills" | "tabs" | "agents">("chat");
  /** P121 — the agent whose notes and runs (outside its channel) the Canais tab lists, instead of the contacts; `null` for the contacts. */
  const [workOf, setWorkOf] = useState<string | null>(null);
  /** P121 — the conversation of that list that the chat tab is showing, to come back to the list from it. */
  const [workOpen, setWorkOpen] = useState<{ agent: string; id: string } | null>(null);

  useEffect(() => {
    chrome.runtime.sendMessage({ type: "getStatus" }).then((res: GetStatusResponse) => {
      setStatus(res.status);
      setHistory(res.history);
      setSavedSettings(res.savedSettings);
      const { conversations, activeConversationId, pendingIds, agentIds, agentId, workdir, threadParent, channels, unreadChannels } = res;
      setConversationState({ conversations, activeConversationId, pendingIds, agentIds, agentId, workdir, threadParent, channels, unreadChannels });
      setApprovals(res.approvals);
    });

    function onEvent(event: BackgroundEvent) {
      if (event.type === "statusChanged") {
        setStatus(event.status);
      } else if (event.type === "chatMessage") {
        setHistory((h) => [...h, event.entry]);
      } else if (event.type === "historyLoaded") {
        setHistory(event.history);
      } else if (event.type === "conversationsChanged") {
        const { conversations, activeConversationId, pendingIds, agentIds, agentId, workdir, threadParent, channels, unreadChannels } = event;
        setConversationState({ conversations, activeConversationId, pendingIds, agentIds, agentId, workdir, threadParent, channels, unreadChannels });
      } else if (event.type === "approvalsChanged") {
        setApprovals(event.approvals);
      }
    }
    chrome.runtime.onMessage.addListener(onEvent);
    return () => chrome.runtime.onMessage.removeListener(onEvent);
  }, []);

  function handleConnect(settings: ConnectionSettings) {
    setConnecting(true);
    setConnectError(undefined);
    chrome.runtime.sendMessage({ type: "connect", ...settings }).then((res: OkResponse) => {
      setConnecting(false);
      if (!res.ok) setConnectError(res.error);
    });
  }

  function handleDisconnect() {
    chrome.runtime.sendMessage({ type: "disconnect" });
    setHistory([]);
  }

  function handleSend(message: string) {
    chrome.runtime.sendMessage({ type: "sendChat", message }).then((res: OkResponse) => {
      if (!res.ok) setHistory((h) => [...h, { role: "error", content: res.error ?? "failed to send" }]);
    });
  }

  /** P120 — a node of the organization tree: an empty conversation with that agent, shown in the chat. */
  async function handleOpenChat(id: string) {
    await chrome.runtime.sendMessage({ type: "newConversation" });
    await chrome.runtime.sendMessage({ type: "selectAgent", agentId: id });
    setTab("chat");
  }

  /** P121 — the agent whose channel the open conversation is, if it is one. */
  const channelAgent = Object.keys(conversationState.channels).find((agent) => conversationState.channels[agent] === conversationState.activeConversationId);

  // P121 — tells the background which channel is in front (the chat tab, showing that channel), so it is read as it changes and does not light
  // the toolbar icon; none when another tab is up. Closing the panel puts it back, so a message that comes then is told.
  const watched = tab === "chat" && channelAgent !== undefined ? conversationState.activeConversationId : null;
  useEffect(() => {
    void chrome.runtime.sendMessage({ type: "watchChannel", conversationId: watched }).catch(() => undefined);
    const release = () => void chrome.runtime.sendMessage({ type: "watchChannel", conversationId: null }).catch(() => undefined);
    window.addEventListener("pagehide", release);
    return () => window.removeEventListener("pagehide", release);
  }, [watched]);

  const pendingChat =conversationState.activeConversationId !== null && conversationState.pendingIds.includes(conversationState.activeConversationId);

  return (
    <div className="sidepanel-app">
      <h1>Warden</h1>
      {status.kind === "connected" ? (
        <>
          <nav className="tab-bar">
            <button type="button" className={tab === "chat" ? "tab tab--active" : "tab"} onClick={() => setTab("chat")}>
              Chat
            </button>
            <button type="button" className={tab === "channels" ? "tab tab--active" : "tab"} onClick={() => setTab("channels")}>
              Canais
              {conversationState.unreadChannels.length > 0 && (
                <span className="tab-badge" aria-label={`${conversationState.unreadChannels.length} com mensagens novas`}>
                  {conversationState.unreadChannels.length}
                </span>
              )}
            </button>
            <button type="button" className={tab === "skills" ? "tab tab--active" : "tab"} onClick={() => setTab("skills")}>
              Skills
            </button>
            <button type="button" className={tab === "agents" ? "tab tab--active" : "tab"} onClick={() => setTab("agents")}>
              Agentes
            </button>
            <button type="button" className={tab === "tabs" ? "tab tab--active" : "tab"} onClick={() => setTab("tabs")}>
              Abas
            </button>
          </nav>
          {/* All three stay mounted (just hidden) so switching tabs never scrolls away or loses a half-typed message. */}
          <div className="tab-panel" hidden={tab !== "chat"}>
            {approvals[0] && <ApprovalCard prompt={approvals[0]} waiting={approvals.length - 1} />}
            {conversationState.threadParent ? (
              <header className="thread-bar">
                <button
                  type="button"
                  className="link-button"
                  onClick={() => void chrome.runtime.sendMessage({ type: "selectConversation", conversationId: conversationState.threadParent!.conversationId })}
                >
                  ← Voltar à conversa
                </button>
                <strong>Thread</strong>
                {(conversationState.agentIds.length > 0 || conversationState.agentId !== null) && (
                  // Each thread talks to the agent the person picks, the main conversation's or another; locked while an answer is on the way.
                  <label className="conversation-bar-agent">
                    Agente
                    <select
                      value={conversationState.agentId ?? ""}
                      disabled={pendingChat}
                      onChange={(e) => void chrome.runtime.sendMessage({ type: "selectAgent", agentId: e.target.value || null })}
                    >
                      <option value="">Nenhum</option>
                      {conversationState.agentIds.map((id) => (
                        <option key={id} value={id}>
                          {id}
                        </option>
                      ))}
                      {conversationState.agentId !== null && !conversationState.agentIds.includes(conversationState.agentId) && (
                        <option value={conversationState.agentId}>{conversationState.agentId} (removido)</option>
                      )}
                    </select>
                  </label>
                )}
              </header>
            ) : workOpen && workOpen.id === conversationState.activeConversationId ? (
              // P121 — a note or a run of an agent, opened from its list: back to the list, not to the loose conversations.
              <header className="thread-bar">
                <button
                  type="button"
                  className="link-button"
                  onClick={() => {
                    setWorkOpen(null);
                    setTab("channels");
                  }}
                >
                  ← Recados e execuções
                </button>
                <strong>{conversationState.conversations.find((c) => c.id === workOpen.id)?.title ?? workOpen.agent}</strong>
              </header>
            ) : channelAgent !== undefined ? (
              // P121 — an agent's channel: the agent is the channel's, there is no agent, folder or conversation to pick here.
              <header className="thread-bar">
                <button
                  type="button"
                  className="link-button"
                  onClick={() => {
                    setWorkOf(null);
                    setTab("channels");
                  }}
                >
                  ← Canais
                </button>
                <strong>{channelAgent}</strong>
                <button
                  type="button"
                  className="link-button"
                  onClick={() => {
                    setWorkOf(channelAgent);
                    setTab("channels");
                  }}
                >
                  {workButtonLabel(agentWork(conversationState.conversations, channelAgent).length)}
                </button>
                <button type="button" className="link-button" onClick={() => void chrome.runtime.sendMessage({ type: "newConversation" })}>
                  Conversa nova
                </button>
              </header>
            ) : (
              <ConversationBar {...conversationState} />
            )}
            <ChatView
              serverName={status.serverName}
              history={history}
              pending={pendingChat}
              threads={conversationState.activeConversationId ? threadsOf(conversationState.conversations, conversationState.activeConversationId) : {}}
              inThread={conversationState.threadParent !== null}
              onOpenThread={(messageId) =>
                conversationState.activeConversationId &&
                void chrome.runtime.sendMessage({ type: "openThread", conversationId: conversationState.activeConversationId, messageId })
              }
              onSend={handleSend}
              onDisconnect={handleDisconnect}
            />
          </div>
          {tab === "channels" && (
            <div className="tab-panel">
              {workOf !== null ? (
                <AgentWorkList
                  agent={workOf}
                  items={agentWork(conversationState.conversations, workOf)}
                  pendingIds={conversationState.pendingIds}
                  onOpen={(conversationId) => {
                    setWorkOpen({ agent: workOf, id: conversationId });
                    void chrome.runtime.sendMessage({ type: "selectConversation", conversationId });
                    setTab("chat");
                  }}
                  onBack={() => setWorkOf(null)}
                />
              ) : (
                <ChannelsView
                  agentIds={conversationState.agentIds}
                  channels={conversationState.channels}
                  conversations={conversationState.conversations}
                  pendingIds={conversationState.pendingIds}
                  unreadChannels={conversationState.unreadChannels}
                  onOpened={() => setTab("chat")}
                />
              )}
            </div>
          )}
          {tab === "skills" && (
            <div className="tab-panel">
              <SkillsView />
            </div>
          )}
          {tab === "agents" && (
            <div className="tab-panel">
              <AgentsView
                onOpenChat={(id) => void handleOpenChat(id)}
                onOpenConversation={(conversationId) => {
                  void chrome.runtime.sendMessage({ type: "selectConversation", conversationId });
                  setTab("chat");
                }}
                onOpenChannel={(agentId) => {
                  void chrome.runtime.sendMessage({ type: "openAgentChannel", agentId }).then(() => setTab("chat"));
                }}
              />
            </div>
          )}
          {tab === "tabs" && (
            <div className="tab-panel">
              <TabsView />
            </div>
          )}
        </>
      ) : (
        <ConnectionForm
          savedSettings={savedSettings}
          errorMessage={connectError ?? (status.kind === "failure" ? status.message : undefined)}
          busy={connecting || status.kind === "connecting"}
          onConnect={handleConnect}
        />
      )}
    </div>
  );
}
