use async_trait::async_trait;

use crate::tool::ToolSpec;

pub mod openai;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub content: String,
}

/// Abstraction implemented by each AI provider (OpenAI, Anthropic, Gemini, local).
/// The orchestrator only ever talks to this trait — never to a concrete provider.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<Response>;
}
