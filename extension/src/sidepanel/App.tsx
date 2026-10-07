import { useEffect, useState } from "react";
import ConnectionForm from "./ConnectionForm";
import ChatView from "./ChatView";
import ConversationBar from "./ConversationBar";
import SkillsView from "./SkillsView";
import AgentsView from "./AgentsView";
import TabsView from "./TabsView";
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
  });
  /** P87 — approvals the hub is waiting on, kept by the background. */
  const [approvals, setApprovals] = useState<ApprovalPrompt[]>([]);
  const [tab, setTab] = useState<"chat" | "skills" | "tabs" | "agents">("chat");

  useEffect(() => {
    chrome.runtime.sendMessage({ type: "getStatus" }).then((res: GetStatusResponse) => {
      setStatus(res.status);
      setHistory(res.history);
      setSavedSettings(res.savedSettings);
      const { conversations, activeConversationId, pendingIds, agentIds, agentId, workdir, threadParent } = res;
      setConversationState({ conversations, activeConversationId, pendingIds, agentIds, agentId, workdir, threadParent });
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
        const { conversations, activeConversationId, pendingIds, agentIds, agentId, workdir, threadParent } = event;
        setConversationState({ conversations, activeConversationId, pendingIds, agentIds, agentId, workdir, threadParent });
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
          {tab === "skills" && (
            <div className="tab-panel">
              <SkillsView />
            </div>
          )}
          {tab === "agents" && (
            <div className="tab-panel">
              <AgentsView onOpenChat={(id) => void handleOpenChat(id)} />
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
