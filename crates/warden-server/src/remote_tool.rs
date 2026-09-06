use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use warden_core::tool::{Tool, ToolSpec};

use crate::protocol::ServerMessage;

/// A model call taking this long to answer is already unusual (Fase 7.3's `Chat` has no timeout
/// at all, since a slow model reply is expected); a *local file read* on a phone taking this long
/// means the client is gone or stuck, not just slow.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

type PendingCalls = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// Shared per-connection state: every `RemoteTool` built for one connection holds a clone of this,
/// so several advertised tools on the same connection share one `call_id` sequence and one pending
/// map instead of colliding.
#[derive(Clone)]
pub struct RemoteToolChannel {
    tx: mpsc::UnboundedSender<ServerMessage>,
    pending: PendingCalls,
    next_call_id: Arc<AtomicU64>,
}

impl RemoteToolChannel {
    pub fn new(tx: mpsc::UnboundedSender<ServerMessage>) -> Self {
        Self { tx, pending: Arc::new(Mutex::new(HashMap::new())), next_call_id: Arc::new(AtomicU64::new(0)) }
    }

    /// Resolves a pending call by id — the connection's read loop calls this on
    /// `ClientMessage::ToolCallResult`/`ToolCallError`. A no-op if the id is unknown (already
    /// timed out, or a stray/duplicate reply), same "ignore what you can't correlate" posture
    /// `ServerConnection` (Dart side) already takes for an unsolicited `Pong`.
    pub fn resolve(&self, call_id: u64, result: Result<Value, String>) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&call_id) {
            let _ = tx.send(result);
        }
    }
}

/// A `Tool` that proxies `call()` over a connection instead of running locally — the mechanism
/// Fase 7.4 needs to give a specific connected client (mobile: file access) a tool the model can
/// invoke, and the first concrete piece of what Fase 9.4/9.5 will later generalize to arbitrary
/// tool routing.
pub struct RemoteTool {
    spec: ToolSpec,
    channel: RemoteToolChannel,
    timeout: Duration,
}

impl RemoteTool {
    pub fn new(spec: ToolSpec, channel: RemoteToolChannel, timeout: Duration) -> Self {
        Self { spec, channel, timeout }
    }
}

#[async_trait]
impl Tool for RemoteTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let call_id = self.channel.next_call_id.fetch_add(1, Ordering::Relaxed);
        let (result_tx, result_rx) = oneshot::channel();
        self.channel.pending.lock().unwrap().insert(call_id, result_tx);

        let request = ServerMessage::ToolCallRequest { call_id, tool: self.spec.name.clone(), arguments: args };
        if self.channel.tx.send(request).is_err() {
            self.channel.pending.lock().unwrap().remove(&call_id);
            anyhow::bail!("'{}' failed: connection closed before the request could be sent", self.spec.name);
        }

        match tokio::time::timeout(self.timeout, result_rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => anyhow::bail!("remote tool '{}' failed: {message}", self.spec.name),
            Ok(Err(_canceled)) => anyhow::bail!("'{}' failed: connection closed while waiting for a reply", self.spec.name),
            Err(_elapsed) => {
                self.channel.pending.lock().unwrap().remove(&call_id);
                anyhow::bail!("remote tool '{}' timed out after {:?}", self.spec.name, self.timeout)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn a_spec() -> ToolSpec {
        ToolSpec { name: "echo".to_string(), description: "echoes".to_string(), parameters: json!({"type": "object"}) }
    }

    #[tokio::test]
    async fn a_successful_reply_resolves_the_call() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let channel = RemoteToolChannel::new(tx);
        let tool = RemoteTool::new(a_spec(), channel.clone(), DEFAULT_TIMEOUT);

        let responder = tokio::spawn(async move {
            match rx.recv().await.unwrap() {
                ServerMessage::ToolCallRequest { call_id, tool, arguments } => {
                    assert_eq!(tool, "echo");
                    assert_eq!(arguments, json!({"x": 1}));
                    channel.resolve(call_id, Ok(json!({"x": 1})));
                }
                other => panic!("expected ToolCallRequest, got {other:?}"),
            }
        });

        let result = tool.call(json!({"x": 1})).await.unwrap();
        assert_eq!(result, json!({"x": 1}));
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn a_client_side_error_surfaces_with_the_real_message() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let channel = RemoteToolChannel::new(tx);
        let tool = RemoteTool::new(a_spec(), channel.clone(), DEFAULT_TIMEOUT);

        tokio::spawn(async move {
            let ServerMessage::ToolCallRequest { call_id, .. } = rx.recv().await.unwrap() else { panic!("expected a request") };
            channel.resolve(call_id, Err("file not found".to_string()));
        });

        let err = tool.call(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("file not found"), "error was: {err}");
    }

    #[tokio::test]
    async fn a_call_that_never_gets_a_reply_times_out() {
        let (tx, _rx) = mpsc::unbounded_channel(); // held so the sender doesn't itself fail
        let channel = RemoteToolChannel::new(tx);
        let tool = RemoteTool::new(a_spec(), channel, Duration::from_millis(50));

        let err = tool.call(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "error was: {err}");
    }

    #[tokio::test]
    async fn a_dropped_connection_fails_fast_instead_of_waiting_for_the_timeout() {
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx); // simulates the writer task/connection already gone
        let channel = RemoteToolChannel::new(tx);
        let tool = RemoteTool::new(a_spec(), channel, DEFAULT_TIMEOUT);

        let err = tool.call(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("connection closed"), "error was: {err}");
    }
}
