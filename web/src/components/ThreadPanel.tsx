import { useCallback, useEffect, useRef, useState } from "react";
import type { ChatEntry, ServerConnection } from "../hub/connection";
import { historyToEntries } from "../hub/connection";
import type { Attachment } from "../hub/messages";
import ChatView from "./ChatView";

const HISTORY_LIMIT = 200;

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** P125 — a thread de uma mensagem: um chat à parte, à direita da conversa, ligado a uma conversa filha dela. O modelo vê a conversa até a
 * mensagem de onde a thread saiu e depois só o que se diz aqui; nada volta para a conversa principal, só o contador de respostas na
 * mensagem. `threadId` é a conversa da thread (a que já existe, ou um id novo que o hub cria com a primeira resposta), e `parent` é a
 * mensagem de onde ela sai, dita só na primeira. */
export default function ThreadPanel({
  conn,
  threadId,
  parent,
  anchor,
  agentIds,
  initialAgentId,
  disabled,
  onClose,
}: {
  conn: ServerConnection;
  threadId: string;
  parent: { conversationId: string; messageId: string };
  /** A mensagem de onde a thread saiu, para o topo do painel. */
  anchor: ChatEntry;
  /** Os agentes configurados no hub, para escolher com quem a thread fala. */
  agentIds: string[];
  /** Com quem a thread começa: o agente com que ela falou da última vez, ou o da conversa principal se é nova. A pessoa troca no painel. */
  initialAgentId: string;
  disabled: boolean;
  onClose: () => void;
}) {
  const [agentId, setAgentId] = useState(initialAgentId);
  const [entries, setEntries] = useState<ChatEntry[]>([]);
  const [pending, setPending] = useState(false);
  const threadRef = useRef(threadId);
  threadRef.current = threadId;

  // O histórico da thread, se ela já existe (uma que ainda não existe vem vazia).
  useEffect(() => {
    let alive = true;
    setEntries([]);
    setPending(false);
    conn.fetchHistory(threadId, HISTORY_LIMIT).then(
      (history) => alive && setEntries(historyToEntries(history)),
      (err) => alive && setEntries([{ role: "error", content: `Não foi possível carregar a thread: ${errorText(err)}`, attachments: [] }]),
    );
    return () => {
      alive = false;
    };
  }, [conn, threadId]);

  // A resposta (ou o erro) da thread chega pela mesma conexão do chat principal, marcada com o id dela.
  useEffect(
    () =>
      conn.onChatMessage((entry, conversationId) => {
        if (conversationId !== threadRef.current) return;
        setPending(false);
        setEntries((current) => [...current, entry]);
      }),
    [conn],
  );

  const send = useCallback(
    (message: string, attachments: Attachment[]) => {
      setEntries((current) => [...current, { role: "user", content: message, attachments }]);
      setPending(true);
      try {
        conn.sendChat(message, threadId, attachments, agentId || undefined, undefined, undefined, parent);
      } catch (err) {
        setPending(false);
        setEntries((current) => [...current, { role: "error", content: errorText(err), attachments: [] }]);
      }
    },
    [conn, threadId, agentId, parent],
  );

  return (
    <aside className="thread-panel" aria-label="Thread">
      <div className="thread-header">
        <strong>Thread</strong>
        {(agentIds.length > 0 || agentId !== "") && (
          // Cada thread fala com o agente que a pessoa escolher, o mesmo da conversa principal ou outro; trava enquanto a resposta não vem.
          <label className="agent-picker">
            <span className="agent-picker-label">Agente</span>
            <select value={agentId} onChange={(e) => setAgentId(e.target.value)} disabled={pending}>
              <option value="">Nenhum</option>
              {agentIds.map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
              {agentId !== "" && !agentIds.includes(agentId) && <option value={agentId}>{agentId} (removido)</option>}
            </select>
          </label>
        )}
        <button type="button" className="link-button" onClick={onClose} aria-label="Fechar a thread">
          Fechar
        </button>
      </div>
      <blockquote className="thread-anchor">
        <span className="skills-hint">{anchor.role === "user" ? "Você" : "Assistente"} disse:</span>
        <p>{anchor.content.length > 400 ? `${anchor.content.slice(0, 400)}…` : anchor.content}</p>
      </blockquote>
      <ChatView
        entries={entries}
        pending={pending}
        onCancel={() => undefined}
        disabled={disabled}
        onSend={send}
        onTranscribe={(audio) => conn.transcribe(audio)}
        onExtendLimit={(limitId) => conn.extendLimit(limitId).then(() => undefined)}
      />
    </aside>
  );
}
