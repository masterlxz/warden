// Shared across multiple test binaries (`handshake.rs`, `chat.rs`); each one only exercises a
// subset of these constructors, so the ones unused by any given binary would otherwise warn.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::Server;

/// A `ModelProvider` test double — no real API key needed, same helper (`response_stream`)
/// `warden-core`'s own unit tests use to turn a canned `Response` into a `ChatStream`.
pub struct MockProvider {
    pub reply: String,
    pub delay: Duration,
    pub fail: bool,
    /// When set, the *first* `chat_stream` call returns a tool call instead of `reply` — every
    /// call after that returns `reply` with no tool calls, same two-round shape
    /// `Orchestrator::handle_turn`'s own tool-calling loop expects (see `warden-core`'s
    /// `orchestrator::tests::MockModel`).
    pub tool_call: Option<(String, Value)>,
    calls: AtomicUsize,
}

impl MockProvider {
    pub fn replying(reply: impl Into<String>) -> Self {
        Self { reply: reply.into(), delay: Duration::ZERO, fail: false, tool_call: None, calls: AtomicUsize::new(0) }
    }

    pub fn failing() -> Self {
        Self { reply: String::new(), delay: Duration::ZERO, fail: true, tool_call: None, calls: AtomicUsize::new(0) }
    }

    pub fn replying_after(reply: impl Into<String>, delay: Duration) -> Self {
        Self { reply: reply.into(), delay, fail: false, tool_call: None, calls: AtomicUsize::new(0) }
    }

    /// First `chat_stream` call requests `tool(arguments)`; the *second* call's reply is derived
    /// straight from the real `Role::Tool` result message the orchestrator fed back — not a
    /// canned string — so a test asserting on the final `ChatResponse.content` is actually proving
    /// the client's real tool result made it all the way back through the model, not just that a
    /// request/response pair of frames was exchanged.
    pub fn calling_tool_then_replying(tool: impl Into<String>, arguments: Value) -> Self {
        Self { reply: String::new(), delay: Duration::ZERO, fail: false, tool_call: Some((tool.into(), arguments)), calls: AtomicUsize::new(0) }
    }
}

#[async_trait]
impl ModelProvider for MockProvider {
    async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if self.fail {
            anyhow::bail!("mock provider failure");
        }

        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((name, arguments)) = &self.tool_call {
            if call == 0 {
                return Ok(response_stream(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".to_string(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                        thought_signature: None,
                    }],
                    usage: None,
                }));
            }
            let tool_result = messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Tool)
                .map(|m| m.content.clone())
                .unwrap_or_default();
            return Ok(response_stream(Response { content: format!("tool said: {tool_result}"), tool_calls: Vec::new(), usage: None }));
        }
        Ok(response_stream(Response { content: self.reply.clone(), tool_calls: Vec::new(), usage: None }))
    }
}

/// Spins up a real `Server` (bound to an OS-assigned localhost port) with `provider` as its
/// model — used by every integration test in this crate so none of them need a real API key.
pub async fn spin_up_server(provider: MockProvider) -> std::net::SocketAddr {
    let temp_dir = std::env::temp_dir().join(format!(
        "warden-server-test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let vault = Arc::new(Vault::new(temp_dir.join("vault")));
    let orchestrator = Orchestrator::new(Arc::new(provider), vault);

    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", Arc::new(orchestrator), temp_dir.join("conversations"))
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    addr
}
