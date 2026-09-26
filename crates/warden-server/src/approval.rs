//! Approvals over the hub connection (P46): a tool in a device's `Chat` turn that needs the person's
//! yes (`manage_agents`, SSH hosts with `require_approval`) asks *that* device — the one whose person
//! is waiting on the answer — the way the desktop shows its modal and the CLI its `[s/N]` card.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use warden_core::tool::{ApprovalRequest, Approver};
use warden_server_protocol::ServerMessage;

/// Same deadline the tools themselves use (`manage_agents`' `APPROVAL_TIMEOUT`): past it the tool has
/// already counted the request as a no, so the prompt is withdrawn from the screen too.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// One per connection. The read loop hands every `ResolveApproval` to `resolve`; when the connection
/// ends, the pending senders are dropped with it and every open request counts as a no.
#[derive(Clone)]
pub struct WsApprover {
    tx: mpsc::UnboundedSender<ServerMessage>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>>,
    next_id: Arc<AtomicU64>,
    timeout: Duration,
}

impl WsApprover {
    pub fn new(tx: mpsc::UnboundedSender<ServerMessage>) -> Self {
        Self { tx, pending: Arc::default(), next_id: Arc::default(), timeout: APPROVAL_TIMEOUT }
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The person's answer to `approval_id`. An id nobody is waiting on (answered, expired, or made
    /// up) is ignored.
    pub fn resolve(&self, approval_id: u64, approved: bool) {
        if let Some(answer) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&approval_id) {
            let _ = answer.send(approved);
        }
    }

    /// Drops every open request (each counts as a no). The connection calls it on its way out, so a
    /// tool doesn't sit out the whole deadline for a device that is already gone.
    pub fn close(&self) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[async_trait]
impl Approver for WsApprover {
    async fn approve(&self, request: ApprovalRequest) -> bool {
        let approval_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer_tx, answer_rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(approval_id, answer_tx);
        let ask = ServerMessage::ApprovalRequest { approval_id, target: request.target, action: request.action, detail: request.detail };
        if self.tx.send(ask).is_err() {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&approval_id);
            return false;
        }
        match tokio::time::timeout(self.timeout, answer_rx).await {
            Ok(Ok(approved)) => approved,
            // The connection closed: nobody can answer any more.
            Ok(Err(_)) => false,
            Err(_) => {
                self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&approval_id);
                let _ = self.tx.send(ServerMessage::ApprovalCancelled { approval_id });
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ApprovalRequest {
        ApprovalRequest { target: "poet".into(), action: "create_agent".into(), detail: "New agent 'poet'".into() }
    }

    /// Answers the first `ApprovalRequest` that reaches the client with `answer`.
    fn answering(approver: &WsApprover, mut rx: mpsc::UnboundedReceiver<ServerMessage>, answer: bool) -> tokio::task::JoinHandle<ServerMessage> {
        let approver = approver.clone();
        tokio::spawn(async move {
            let msg = rx.recv().await.unwrap();
            let ServerMessage::ApprovalRequest { approval_id, .. } = &msg else { panic!("unexpected {msg:?}") };
            approver.resolve(*approval_id, answer);
            msg
        })
    }

    #[tokio::test]
    async fn the_device_answer_is_the_tools_answer() {
        for answer in [true, false] {
            let (tx, rx) = mpsc::unbounded_channel();
            let approver = WsApprover::new(tx);
            let client = answering(&approver, rx, answer);
            assert_eq!(approver.approve(request()).await, answer);
            let asked = client.await.unwrap();
            assert_eq!(
                asked,
                ServerMessage::ApprovalRequest { approval_id: 0, target: "poet".into(), action: "create_agent".into(), detail: "New agent 'poet'".into() }
            );
        }
    }

    #[tokio::test]
    async fn no_answer_in_time_is_a_no_and_the_prompt_is_withdrawn() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let approver = WsApprover::new(tx).with_timeout(Duration::from_millis(50));
        assert!(!approver.approve(request()).await);
        assert!(matches!(rx.recv().await, Some(ServerMessage::ApprovalRequest { approval_id: 0, .. })));
        assert_eq!(rx.recv().await, Some(ServerMessage::ApprovalCancelled { approval_id: 0 }));
        // A late answer changes nothing.
        approver.resolve(0, true);
    }

    #[tokio::test]
    async fn a_closed_connection_is_a_no_without_waiting() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let approver = WsApprover::new(tx);
        let closer = approver.clone();
        tokio::spawn(async move {
            rx.recv().await.unwrap();
            closer.close();
        });
        let started = std::time::Instant::now();
        assert!(!approver.approve(request()).await);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_device_that_is_gone_is_a_no() {
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        assert!(!WsApprover::new(tx).approve(request()).await);
    }
}
