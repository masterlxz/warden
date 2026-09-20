//! Asking the user for a "yes" from inside the desktop window (P47 SSH hosts with
//! `require_approval`, P46 `manage_agents`): a tool calls `Approver::approve`, `TauriApprover` emits an
//! `approval-request` event the frontend turns into a modal, and the modal answers through the
//! `resolve_approval` command. Anything unanswered counts as a refusal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;
use warden_core::tool::{ApprovalRequest, Approver};

/// Requests waiting on the user's answer, keyed by the id the frontend sends back through
/// `resolve_approval`. One broker for the whole app (`AppState`), shared by every turn.
#[derive(Default)]
pub struct ApprovalBroker {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
}

impl ApprovalBroker {
    /// Answers request `id`. `false` when nothing is waiting on it any more (already answered, or
    /// the tool gave up first) — the frontend just closes its modal either way.
    pub fn resolve(&self, id: u64, approved: bool) -> bool {
        match self.pending.lock().unwrap().remove(&id) {
            Some(reply) => reply.send(approved).is_ok(),
            None => false,
        }
    }
}

/// What the modal shows. `camelCase` like every other payload the frontend reads.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPayload {
    pub id: u64,
    pub target: String,
    pub action: String,
    pub detail: String,
}

/// Asks through the desktop window: emits `approval-request`, then waits for
/// `resolve_approval`. If the tool stops waiting first (its own deadline) the guard below
/// forgets the request and emits `approval-cancelled` so the modal doesn't outlive it.
pub struct TauriApprover {
    pub app: AppHandle,
    pub broker: Arc<ApprovalBroker>,
}

struct PendingGuard {
    app: AppHandle,
    broker: Arc<ApprovalBroker>,
    id: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if self.broker.pending.lock().unwrap().remove(&self.id).is_some() {
            let _ = self.app.emit("approval-cancelled", self.id);
        }
    }
}

#[async_trait::async_trait]
impl Approver for TauriApprover {
    async fn approve(&self, request: ApprovalRequest) -> bool {
        let id = self.broker.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.broker.pending.lock().unwrap().insert(id, reply);
        let _guard = PendingGuard { app: self.app.clone(), broker: self.broker.clone(), id };
        let payload = ApprovalPayload { id, target: request.target, action: request.action, detail: request.detail };
        if self.app.emit("approval-request", payload).is_err() {
            return false;
        }
        answer.await.unwrap_or(false)
    }
}

#[tauri::command]
pub fn resolve_approval(state: State<'_, crate::AppState>, id: u64, approved: bool) -> bool {
    state.approvals.resolve(id, approved)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_broker_answers_a_waiting_request_once() {
        let broker = ApprovalBroker::default();
        let (reply, mut answer) = oneshot::channel();
        broker.pending.lock().unwrap().insert(7, reply);

        assert!(broker.resolve(7, true));
        assert_eq!(answer.try_recv(), Ok(true));
        // A second answer, or one for a request that never existed, finds nobody waiting.
        assert!(!broker.resolve(7, false));
        assert!(!broker.resolve(99, true));
    }
}
