import type { ApprovalPrompt } from "../protocol/messages";

interface Props {
  prompt: ApprovalPrompt;
  /** How many more are queued behind this one. */
  waiting: number;
}

/** P87 — a tool in this device's turn waits on the person (an agent creating or editing another, an
 * SSH host with approval, a spending-limit pause). The hub gives up after 120 s, which removes it. */
export default function ApprovalCard({ prompt, waiting }: Props) {
  function answer(approved: boolean) {
    void chrome.runtime.sendMessage({ type: "resolveApproval", approvalId: prompt.approvalId, approved });
  }

  return (
    <section className="approval-card" role="alertdialog" aria-label="Aprovação pedida">
      <strong>Aprovação pedida</strong>
      <p>
        {prompt.action}: <code>{prompt.target}</code>
      </p>
      {prompt.category && <p className="approval-detail">Categoria: {prompt.category} (este agente pede aprovação para isso)</p>}
      {prompt.detail && <p className="approval-detail">{prompt.detail}</p>}
      {waiting > 0 && <p className="approval-detail">Mais {waiting} esperando.</p>}
      <div className="approval-actions">
        <button type="button" onClick={() => answer(true)}>
          Aprovar
        </button>
        <button type="button" className="link-button" onClick={() => answer(false)}>
          Recusar
        </button>
      </div>
    </section>
  );
}
