import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ApprovalRequest } from "../types";

/** Asks the user to approve something the AI wants to do (P47: an SSH action on a server with "Ask me
 * before every command" on; P46: creating or changing an agent). Requests queue up (a turn may run several tool calls); the oldest shows first.
 * Closing it any way other than "Approve" is a refusal, and the backend also treats no answer as
 * one, so a stray click can't approve anything. */
function ApprovalModal() {
  const [queue, setQueue] = useState<ApprovalRequest[]>([]);

  useEffect(() => {
    let disposed = false;
    const unlisten: Array<() => void> = [];
    (async () => {
      const onRequest = await listen<ApprovalRequest>("approval-request", (event) => {
        setQueue((q) => [...q, event.payload]);
      });
      // The backend stopped waiting (its own deadline), so the question is moot.
      const onCancelled = await listen<number>("approval-cancelled", (event) => {
        setQueue((q) => q.filter((r) => r.id !== event.payload));
      });
      if (disposed) {
        onRequest();
        onCancelled();
      } else {
        unlisten.push(onRequest, onCancelled);
      }
    })();
    return () => {
      disposed = true;
      unlisten.forEach((fn) => fn());
    };
  }, []);

  const current = queue[0];
  if (!current) return null;

  async function answer(approved: boolean) {
    setQueue((q) => q.filter((r) => r.id !== current.id));
    try {
      await invoke("resolve_approval", { id: current.id, approved });
    } catch {
      // Nothing to do: the request is already gone from the queue, and an unanswered one is refused.
    }
  }

  const verb: Record<string, string> = {
    exec: "run a command on",
    upload: "upload a file to",
    download: "download a file from",
    create_agent: "create the agent",
    update_agent: "change the agent",
  };

  return (
    <div className="settings-modal-backdrop" role="dialog" aria-modal="true" aria-labelledby="approval-title">
      <div className="sync-qr-card settings-modal-card approval-card">
        <h2 id="approval-title" className="approval-title">
          The AI wants to {verb[current.action] ?? current.action} {current.target}
        </h2>
        <pre className="approval-detail">{current.detail}</pre>
        {queue.length > 1 && <p className="settings-hint">{queue.length - 1} more waiting after this one.</p>}
        <div className="approval-actions">
          <button type="button" className="settings-browse-btn" onClick={() => answer(false)} autoFocus>
            Deny
          </button>
          <button type="button" className="settings-save-btn" onClick={() => answer(true)}>
            Approve
          </button>
        </div>
      </div>
    </div>
  );
}

export default ApprovalModal;
