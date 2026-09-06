// Shared across multiple test binaries (`handshake.rs`, `chat.rs`); each one only exercises a
// subset of these constructors, so the ones unused by any given binary would otherwise warn.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::Server;

/// A `ModelProvider` test double — no real API key needed, same helper (`response_stream`)
/// `warden-core`'s own unit tests use to turn a canned `Response` into a `ChatStream`.
pub struct MockProvider {
    pub reply: String,
    pub delay: Duration,
    pub fail: bool,
}

impl MockProvider {
    pub fn replying(reply: impl Into<String>) -> Self {
        Self { reply: reply.into(), delay: Duration::ZERO, fail: false }
    }

    pub fn failing() -> Self {
        Self { reply: String::new(), delay: Duration::ZERO, fail: true }
    }

    pub fn replying_after(reply: impl Into<String>, delay: Duration) -> Self {
        Self { reply: reply.into(), delay, fail: false }
    }
}

#[async_trait]
impl ModelProvider for MockProvider {
    async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if self.fail {
            anyhow::bail!("mock provider failure");
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
