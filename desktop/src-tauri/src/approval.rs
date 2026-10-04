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
use warden_core::tool::{Answer, ApprovalRequest, Approver};

/// Requests waiting on the user's answer, keyed by the id the frontend sends back through
/// `resolve_approval`. One broker for the whole app (`AppState`), shared by every turn.
#[derive(Default)]
pub struct ApprovalBroker {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
}

impl ApprovalBroker {
    /// A fresh id in the same space the frontend answers with, for an approval that comes from a hub (P102): its own ids
    /// are per connection and would collide with these.
    pub fn allocate(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Answers request `id`. `false` when nothing is waiting on it any more (already answered, or
    /// the tool gave up first) — the frontend just closes its modal either way. `always` counts
    /// only with a yes, and only on an ask that offered it (P103 b).
    pub fn resolve(&self, id: u64, approved: bool, always: bool) -> bool {
        let answer = match (approved, always) {
            (false, _) => Answer::Reject,
            (true, false) => Answer::Once,
            (true, true) => Answer::Always,
        };
        match self.pending.lock().unwrap().remove(&id) {
            Some(reply) => reply.send(answer).is_ok(),
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
    /// What an "always" answer would cover (P103 b), when the ask can be answered that way; the modal then offers it.
    pub always: Option<String>,
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
        self.ask(request, None).await != Answer::Reject
    }

    async fn ask(&self, request: ApprovalRequest, always: Option<&str>) -> Answer {
        let id = self.broker.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.broker.pending.lock().unwrap().insert(id, reply);
        let _guard = PendingGuard { app: self.app.clone(), broker: self.broker.clone(), id };
        let payload = ApprovalPayload { id, target: request.target, action: request.action, detail: request.detail, always: always.map(str::to_string) };
        if self.app.emit("approval-request", payload).is_err() {
            return Answer::Reject;
        }
        answer.await.unwrap_or(Answer::Reject)
    }
}

#[tauri::command]
pub fn resolve_approval(state: State<'_, crate::AppState>, id: u64, approved: bool, always: Option<bool>) -> bool {
    let always = always.unwrap_or(false);
    // An approval a hub asked for (P102) is answered to that hub, under the hub's id.
    let remote = state.remote.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|s| {
        let hub_id = s.approvals.lock().unwrap_or_else(|e| e.into_inner()).take(id)?;
        Some((s.handle.clone(), hub_id))
    });
    if let Some((handle, approval_id)) = remote {
        handle.send(warden_server::ClientMessage::ResolveApproval { approval_id, approved, always: approved && always });
        return true;
    }
    state.approvals.resolve(id, approved, always)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_broker_answers_a_waiting_request_once() {
        let broker = ApprovalBroker::default();
        let (reply, mut answer) = oneshot::channel();
        broker.pending.lock().unwrap().insert(7, reply);

        assert!(broker.resolve(7, true, false));
        assert_eq!(answer.try_recv(), Ok(Answer::Once));
        // A second answer, or one for a request that never existed, finds nobody waiting.
        assert!(!broker.resolve(7, false, false));
        assert!(!broker.resolve(99, true, false));
    }

    #[test]
    fn always_counts_only_with_a_yes() {
        for (approved, always, expected) in [(true, true, Answer::Always), (false, true, Answer::Reject), (false, false, Answer::Reject)] {
            let broker = ApprovalBroker::default();
            let (reply, mut answer) = oneshot::channel();
            broker.pending.lock().unwrap().insert(1, reply);
            assert!(broker.resolve(1, approved, always));
            assert_eq!(answer.try_recv(), Ok(expected), "approved={approved} always={always}");
        }
    }
}
