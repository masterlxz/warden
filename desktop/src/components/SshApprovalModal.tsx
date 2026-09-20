import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { SshApprovalRequest } from "../types";

/** Asks the user to approve an SSH action on a server that has "Ask me before every command"
 * switched on (P47). Requests queue up (a turn may run several tool calls); the oldest shows first.
 * Closing it any way other than "Approve" is a refusal, and the backend also treats no answer as
 * one, so a stray click can't approve anything. */
function SshApprovalModal() {
  const [queue, setQueue] = useState<SshApprovalRequest[]>([]);

  useEffect(() => {
    let disposed = false;
    const unlisten: Array<() => void> = [];
    (async () => {
      const onRequest = await listen<SshApprovalRequest>("ssh-approval-request", (event) => {
        setQueue((q) => [...q, event.payload]);
      });
      // The backend stopped waiting (its own deadline), so the question is moot.
      const onCancelled = await listen<number>("ssh-approval-cancelled", (event) => {
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
      await invoke("resolve_ssh_approval", { id: current.id, approved });
    } catch {
      // Nothing to do: the request is already gone from the queue, and an unanswered one is refused.
    }
  }

  const verb: Record<string, string> = { exec: "run a command on", upload: "upload a file to", download: "download a file from" };

  return (
    <div className="settings-modal-backdrop" role="dialog" aria-modal="true" aria-labelledby="ssh-approval-title">
      <div className="sync-qr-card settings-modal-card ssh-approval-card">
        <h2 id="ssh-approval-title" className="ssh-approval-title">
          The AI wants to {verb[current.action] ?? current.action} {current.hostId}
        </h2>
        <pre className="ssh-approval-detail">{current.detail}</pre>
        {queue.length > 1 && <p className="settings-hint">{queue.length - 1} more waiting after this one.</p>}
        <div className="ssh-approval-actions">
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

export default SshApprovalModal;
