import { useEffect, useState } from "react";
import ConnectionForm from "./ConnectionForm";
import ChatView from "./ChatView";
import type { ChatEntry, ConnectionStatus } from "../background/connection";
import type { BackgroundEvent, ConnectionSettings, GetStatusResponse, OkResponse } from "../background/popup_protocol";

export default function App() {
  const [status, setStatus] = useState<ConnectionStatus>({ kind: "disconnected" });
  const [history, setHistory] = useState<ChatEntry[]>([]);
  const [savedSettings, setSavedSettings] = useState<Partial<ConnectionSettings>>({});
  const [connectError, setConnectError] = useState<string | undefined>(undefined);
  const [connecting, setConnecting] = useState(false);
  const [pendingChat, setPendingChat] = useState(false);

  useEffect(() => {
    chrome.runtime.sendMessage({ type: "getStatus" }).then((res: GetStatusResponse) => {
      setStatus(res.status);
      setHistory(res.history);
      setSavedSettings(res.savedSettings);
    });

    function onEvent(event: BackgroundEvent) {
      if (event.type === "statusChanged") {
        setStatus(event.status);
      } else if (event.type === "chatMessage") {
        setHistory((h) => [...h, event.entry]);
        // The background echoes the user's own outgoing message back too (see
        // `background/index.ts`'s `sendChat` case) — only a real reply clears "waiting".
        if (event.entry.role !== "user") setPendingChat(false);
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
    setPendingChat(true);
    chrome.runtime.sendMessage({ type: "sendChat", message }).then((res: OkResponse) => {
      if (!res.ok) {
        setPendingChat(false);
        setHistory((h) => [...h, { role: "error", content: res.error ?? "failed to send" }]);
      }
    });
  }

  return (
    <div className="sidepanel-app">
      <h1>Warden</h1>
      {status.kind === "connected" ? (
        <ChatView serverName={status.serverName} history={history} pending={pendingChat} onSend={handleSend} onDisconnect={handleDisconnect} />
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
