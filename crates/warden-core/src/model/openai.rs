use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Message, ModelProvider, Response, Role, ToolCall};
use crate::tool::ToolSpec;

const API_URL: &str = "https://api.openai.com/v1/chat/completions";

pub struct OpenAiProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatTool>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OutgoingToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct OutgoingToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: OutgoingFunctionCall,
}

#[derive(Serialize)]
struct OutgoingFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct ChatTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: ChatToolFunction,
}

#[derive(Serialize)]
struct ChatToolFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<IncomingToolCall>,
}

#[derive(Deserialize)]
struct IncomingToolCall {
    id: String,
    function: IncomingFunctionCall,
}

#[derive(Deserialize)]
struct IncomingFunctionCall {
    name: String,
    arguments: String,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn to_chat_message(message: Message) -> ChatMessage {
    if message.role == Role::Tool {
        return ChatMessage {
            role: "tool",
            content: Some(message.content),
            tool_calls: None,
            tool_call_id: message.tool_call_id,
        };
    }

    if message.role == Role::Assistant && !message.tool_calls.is_empty() {
        return ChatMessage {
            role: "assistant",
            content: if message.content.is_empty() { None } else { Some(message.content) },
            tool_calls: Some(
                message
                    .tool_calls
                    .into_iter()
                    .map(|tc| OutgoingToolCall {
                        id: tc.id,
                        kind: "function",
                        function: OutgoingFunctionCall {
                            name: tc.name,
                            arguments: tc.arguments.to_string(),
                        },
                    })
                    .collect(),
            ),
            tool_call_id: None,
        };
    }

    ChatMessage { role: role_str(message.role), content: Some(message.content), tool_calls: None, tool_call_id: None }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: messages.into_iter().map(to_chat_message).collect(),
            tools: tools
                .into_iter()
                .map(|t| ChatTool {
                    kind: "function",
                    function: ChatToolFunction {
                        name: t.name,
                        description: t.description,
                        parameters: t.parameters,
                    },
                })
                .collect(),
        };

        let response = self
            .client
            .post(API_URL)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error ({status}): {body}");
        }

        let parsed: ChatResponse = response.json().await?;
        let message = parsed.choices.into_iter().next().map(|c| c.message);

        let content = message.as_ref().and_then(|m| m.content.clone()).unwrap_or_default();
        let tool_calls = message
            .map(|m| {
                m.tool_calls
                    .into_iter()
                    .map(|tc| ToolCall {
                        id: tc.id,
                        name: tc.function.name,
                        arguments: serde_json::from_str(&tc.function.arguments).unwrap_or(Value::Null),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Response { content, tool_calls })
    }
}
