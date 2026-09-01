use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Attachment, Message, ModelProvider, Response, Role, ToolCall, Usage};
use crate::tool::ToolSpec;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiProvider {
    api_key: String,
    model: String,
    /// Root of the API, without a trailing slash — `chat()` appends `/chat/completions`. Lets
    /// this same provider talk to any OpenAI-compatible server (Ollama, OpenRouter, Groq, ...)
    /// by pointing it elsewhere instead of hardcoding OpenAI's own endpoint.
    base_url: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::with_base_url(api_key, model, DEFAULT_BASE_URL)
    }

    /// For any OpenAI-compatible server that isn't OpenAI itself — e.g. Ollama
    /// (`http://localhost:11434/v1`), which needs no real API key.
    pub fn with_base_url(api_key: impl Into<String>, model: impl Into<String>, base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.trim_end_matches('/').to_string(),
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
    content: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OutgoingToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// A message's `content` is normally just a string, but the chat-completions wire format also
/// accepts an array of parts (text + images) for multimodal user turns (P28) — `untagged` picks
/// whichever shape matches what's actually being sent.
#[derive(Serialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Serialize)]
struct ImageUrl {
    url: String,
}

fn attachment_part(attachment: Attachment) -> ContentPart {
    ContentPart::ImageUrl { image_url: ImageUrl { url: format!("data:{};base64,{}", attachment.mime_type, attachment.data) } }
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
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
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
            content: Some(Content::Text(message.content)),
            tool_calls: None,
            tool_call_id: message.tool_call_id,
        };
    }

    if message.role == Role::Assistant && !message.tool_calls.is_empty() {
        return ChatMessage {
            role: "assistant",
            content: if message.content.is_empty() { None } else { Some(Content::Text(message.content)) },
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

    let content = if message.attachments.is_empty() {
        Content::Text(message.content)
    } else {
        let mut parts = vec![ContentPart::Text { text: message.content }];
        parts.extend(message.attachments.into_iter().map(attachment_part));
        Content::Parts(parts)
    };

    ChatMessage { role: role_str(message.role), content: Some(content), tool_calls: None, tool_call_id: None }
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
            .post(format!("{}/chat/completions", self.base_url))
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
        let usage = parsed.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });
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

        Ok(Response { content, tool_calls, usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_user_message_serializes_content_as_a_string() {
        let json = serde_json::to_value(to_chat_message(Message::user("hi"))).unwrap();
        assert_eq!(json["content"], "hi");
    }

    #[test]
    fn attachments_turn_content_into_text_and_image_url_parts() {
        let message = Message::user_with_attachments("what's this?", vec![Attachment { mime_type: "image/png".to_string(), data: "AAAA".to_string() }]);
        let json = serde_json::to_value(to_chat_message(message)).unwrap();

        let parts = json["content"].as_array().expect("content should be an array when attachments are present");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "what's this?");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }
}
