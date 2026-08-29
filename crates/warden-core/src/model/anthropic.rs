use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Message, ModelProvider, Response, Role, ToolCall, Usage};
use crate::tool::ToolSpec;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic's Messages API has no concept of an open-ended response — every request must name
/// a cap. There's no per-request config for it yet (same "no rate limiting/spend cap" scope as
/// the rest of Fase 5.8's `Usage` tracking), so this is just a generous fixed value.
const MAX_TOKENS: u32 = 4096;

pub struct AnthropicProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self { api_key: api_key.into(), model: model.into(), client: reqwest::Client::new() }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String },
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: Vec<ContentBlock>,
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
}

#[derive(Deserialize, Default)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ResponseBlock>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
    /// Catches block types this integration doesn't act on (e.g. `thinking`,
    /// `redacted_thinking`) instead of failing the whole response to parse.
    #[serde(other)]
    Other,
}

fn to_anthropic_message(message: Message) -> AnthropicMessage {
    match message.role {
        Role::System => unreachable!("system messages are pulled out into `system` before this point"),
        // Anthropic has no dedicated tool-result role — a tool result is a `user` message
        // carrying a `tool_result` content block instead.
        Role::Tool => AnthropicMessage {
            role: "user",
            content: vec![ContentBlock::ToolResult { tool_use_id: message.tool_call_id.unwrap_or_default(), content: message.content }],
        },
        Role::Assistant if !message.tool_calls.is_empty() => AnthropicMessage {
            role: "assistant",
            content: message
                .tool_calls
                .into_iter()
                .map(|tc| ContentBlock::ToolUse { id: tc.id, name: tc.name, input: tc.arguments })
                .collect(),
        },
        Role::Assistant => AnthropicMessage { role: "assistant", content: vec![ContentBlock::Text { text: message.content }] },
        Role::User => AnthropicMessage { role: "user", content: vec![ContentBlock::Text { text: message.content }] },
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
        let mut system = None;
        let mut anthropic_messages = Vec::new();
        for message in messages {
            if message.role == Role::System {
                system = Some(message.content);
            } else {
                anthropic_messages.push(to_anthropic_message(message));
            }
        }

        let request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: MAX_TOKENS,
            system,
            messages: anthropic_messages,
            tools: tools.into_iter().map(|t| AnthropicTool { name: t.name, description: t.description, input_schema: t.parameters }).collect(),
        };

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error ({status}): {body}");
        }

        let parsed: MessagesResponse = response.json().await?;
        let usage = parsed.usage.map(|u| Usage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        });

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        for block in parsed.content {
            match block {
                ResponseBlock::Text { text } => content.push_str(&text),
                ResponseBlock::ToolUse { id, name, input } => tool_calls.push(ToolCall { id, name, arguments: input }),
                ResponseBlock::Other => {}
            }
        }

        Ok(Response { content, tool_calls, usage })
    }
}
