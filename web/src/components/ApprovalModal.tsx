import type { ApprovalPrompt } from "../hub/connection";

interface Props {
  /** Oldest first; the first one is shown. */
  queue: ApprovalPrompt[];
  onAnswer: (approvalId: number, approved: boolean, always?: boolean) => void;
}

const VERB: Record<string, string> = {
  exec: "rodar um comando em",
  upload: "enviar um arquivo para",
  download: "baixar um arquivo de",
  create_agent: "criar o agente",
  update_agent: "alterar o agente",
  delete_agent: "apagar o agente",
};

/** Asks the person to approve what the AI wants to do in this browser's turn (P46: creating or
 * changing an agent; SSH hosts that ask first; a spending limit that ran out). Same card as the
 * desktop's `ApprovalModal`. Anything but "Aprovar" is a refusal, and the hub counts no answer as
 * one too, so a stray click can't approve anything. */
export default function ApprovalModal({ queue, onAnswer }: Props) {
  const current = queue[0];
  if (!current) return null;
  // A paused turn (P4) is not the AI asking for something: a limit ran out and the turn waits.
  const spendPause = current.action === "extend_limit";

  return (
    <div className="approval-backdrop" role="dialog" aria-modal="true" aria-labelledby="approval-title">
      <div className="approval-card">
        <h2 id="approval-title" className="approval-title">
          {spendPause ? `Limite de gasto atingido: ${current.target}` : `A IA quer ${VERB[current.action] ?? current.action} ${current.target}`}
        </h2>
        <pre className="approval-detail">{current.detail}</pre>
        {queue.length > 1 && <p className="approval-more">Mais {queue.length - 1} esperando depois deste.</p>}
        <div className="approval-actions">
          <button type="button" className="link-button" onClick={() => onAnswer(current.approvalId, false)} autoFocus>
            {spendPause ? "Parar" : "Recusar"}
          </button>
          <button type="button" className="primary-button" onClick={() => onAnswer(current.approvalId, true)}>
            {spendPause ? "Liberar mais" : "Aprovar"}
          </button>
        </div>
        {current.always && (
          <div className="approval-actions">
            <button type="button" className="link-button" onClick={() => onAnswer(current.approvalId, true, true)} title="Até o hub ser reiniciado">
              Sempre permitir nesta conversa: {current.always}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
