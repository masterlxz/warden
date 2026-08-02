use async_trait::async_trait;
use serde_json::Value;

use crate::tool::ToolSpec;

pub mod gemini;
pub mod openai;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    /// Result of a tool call, fed back to the model.
    Tool,
}

/// A tool invocation requested by the model — either present on an assistant
/// `Message` (what the model asked to run) or standalone in a `Response`
/// (what the provider just parsed out of the model's reply).
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Only set on `Role::Assistant` messages that requested tool calls.
    pub tool_calls: Vec<ToolCall>,
    /// Only set on `Role::Tool` messages: which call this is answering.
    pub tool_call_id: Option<String>,
    /// Only set on `Role::Tool` messages: the name of the tool that ran
    /// (some providers, e.g. Gemini, key tool results by name rather than id).
    pub tool_name: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into(), tool_calls: Vec::new(), tool_call_id: None, tool_name: None }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into(), tool_calls: Vec::new(), tool_call_id: None, tool_name: None }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: content.into(), tool_calls: Vec::new(), tool_call_id: None, tool_name: None }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self { role: Role::Assistant, content: String::new(), tool_calls, tool_call_id: None, tool_name: None }
    }

    pub fn tool_result(tool_call: &ToolCall, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call.id.clone()),
            tool_name: Some(tool_call.name.clone()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Response {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

/// Abstraction implemented by each AI provider (OpenAI, Anthropic, Gemini, local).
/// The orchestrator only ever talks to this trait — never to a concrete provider.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<Response>;
}
