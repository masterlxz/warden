import { useEffect, useState } from "react";
import ConnectionForm from "./ConnectionForm";
import ChatView from "./ChatView";
import ConversationBar from "./ConversationBar";
import SkillsView from "./SkillsView";
import TabsView from "./TabsView";
import type { ChatEntry, ConnectionStatus } from "../background/connection";
import type { BackgroundEvent, ConnectionSettings, ConversationState, GetStatusResponse, OkResponse } from "../background/popup_protocol";

export default function App() {
  const [status, setStatus] = useState<ConnectionStatus>({ kind: "disconnected" });
  const [history, setHistory] = useState<ChatEntry[]>([]);
  const [savedSettings, setSavedSettings] = useState<Partial<ConnectionSettings>>({});
  const [connectError, setConnectError] = useState<string | undefined>(undefined);
  const [connecting, setConnecting] = useState(false);
  /** P78 — kept by the background, which also knows what's waiting on an answer. */
  const [conversationState, setConversationState] = useState<ConversationState>({ conversations: [], activeConversationId: null, pendingIds: [] });
  const [tab, setTab] = useState<"chat" | "skills" | "tabs">("chat");

  useEffect(() => {
    chrome.runtime.sendMessage({ type: "getStatus" }).then((res: GetStatusResponse) => {
      setStatus(res.status);
      setHistory(res.history);
      setSavedSettings(res.savedSettings);
      setConversationState({ conversations: res.conversations, activeConversationId: res.activeConversationId, pendingIds: res.pendingIds });
    });

    function onEvent(event: BackgroundEvent) {
      if (event.type === "statusChanged") {
        setStatus(event.status);
      } else if (event.type === "chatMessage") {
        setHistory((h) => [...h, event.entry]);
      } else if (event.type === "historyLoaded") {
        setHistory(event.history);
      } else if (event.type === "conversationsChanged") {
        setConversationState({ conversations: event.conversations, activeConversationId: event.activeConversationId, pendingIds: event.pendingIds });
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

  const pendingChat = conversationState.activeConversationId !== null && conversationState.pendingIds.includes(conversationState.activeConversationId);

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
            <button type="button" className={tab === "tabs" ? "tab tab--active" : "tab"} onClick={() => setTab("tabs")}>
              Abas
            </button>
          </nav>
          {/* All three stay mounted (just hidden) so switching tabs never scrolls away or loses a half-typed message. */}
          <div className="tab-panel" hidden={tab !== "chat"}>
            <ConversationBar {...conversationState} />
            <ChatView serverName={status.serverName} history={history} pending={pendingChat} onSend={handleSend} onDisconnect={handleDisconnect} />
          </div>
          {tab === "skills" && (
            <div className="tab-panel">
              <SkillsView />
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
