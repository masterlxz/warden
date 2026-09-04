use async_stream::try_stream;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Attachment, ChatStream, Message, ModelProvider, Role, StreamEvent, Usage};
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
    stream: bool,
    stream_options: StreamOptions,
}

/// Without `include_usage: true`, OpenAI's `stream: true` responses never carry a `usage` block
/// at all — the token accounting `chat()`'s callers rely on (persisted by `warden-bootstrap`,
/// shown in the CLI/Telegram footer) would silently become `None` for every streaming call.
#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
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
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

/// Shape of one SSE `data:` chunk under `stream: true`. `choices` is empty on the final
/// usage-only chunk (only sent when `stream_options.include_usage` is set).
#[derive(Deserialize, Default)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolCall>,
}

/// `id`/`function.name` arrive once, on the first fragment for a given `index`; `function.
/// arguments` arrives split across possibly many fragments at that same index, concatenated
/// (never re-parsed) until the stream ends — see `ResponseAccumulator`.
#[derive(Deserialize)]
struct StreamToolCall {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFunctionCall>,
}

#[derive(Deserialize, Default)]
struct StreamFunctionCall {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Maps one SSE `data:` payload to zero or more `StreamEvent`s. Split out from `chat_stream` so
/// it can be unit-tested directly against canned chunk bodies, with no HTTP involved.
fn map_stream_chunk(data: &str) -> anyhow::Result<Vec<StreamEvent>> {
    let chunk: StreamChunk = serde_json::from_str(data)?;
    let mut events = Vec::new();

    if let Some(choice) = chunk.choices.into_iter().next() {
        if let Some(content) = choice.delta.content {
            if !content.is_empty() {
                events.push(StreamEvent::ContentDelta(content));
            }
        }
        for tc in choice.delta.tool_calls {
            events.push(StreamEvent::ToolCallDelta {
                index: tc.index,
                id: tc.id,
                name: tc.function.as_ref().and_then(|f| f.name.clone()),
                arguments_delta: tc.function.and_then(|f| f.arguments),
                thought_signature: None,
            });
        }
    }

    if let Some(usage) = chunk.usage {
        events.push(StreamEvent::Usage(Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }));
    }

    Ok(events)
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
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
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
            stream: true,
            stream_options: StreamOptions { include_usage: true },
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

        Ok(Box::pin(try_stream! {
            let mut frames = response.bytes_stream().eventsource();
            while let Some(frame) = frames.next().await {
                let frame = frame?;
                if frame.data == "[DONE]" {
                    break;
                }
                for event in map_stream_chunk(&frame.data)? {
                    yield event;
                }
            }
        }))
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

    #[test]
    fn a_content_delta_chunk_maps_to_one_content_delta_event() {
        let events = map_stream_chunk(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::ContentDelta(s) if s == "Hel"));
    }

    #[test]
    fn an_empty_content_delta_produces_no_event() {
        let events = map_stream_chunk(r#"{"choices":[{"delta":{}}]}"#).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn a_tool_call_delta_split_across_two_chunks_carries_id_and_name_only_on_the_first() {
        let first = map_stream_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"search","arguments":""}}]}}]}"#).unwrap();
        assert_eq!(first.len(), 1);
        match &first[0] {
            StreamEvent::ToolCallDelta { index, id, name, arguments_delta, .. } => {
                assert_eq!(*index, 0);
                assert_eq!(id.as_deref(), Some("call_1"));
                assert_eq!(name.as_deref(), Some("search"));
                assert_eq!(arguments_delta.as_deref(), Some(""));
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }

        let second = map_stream_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\":1}"}}]}}]}"#).unwrap();
        match &second[0] {
            StreamEvent::ToolCallDelta { id, name, arguments_delta, .. } => {
                assert_eq!(*id, None);
                assert_eq!(*name, None);
                assert_eq!(arguments_delta.as_deref(), Some(r#"{"q":1}"#));
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
    }

    #[test]
    fn a_usage_only_chunk_with_no_choices_maps_to_one_usage_event() {
        let events = map_stream_chunk(r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Usage(u) if u.total_tokens == 15));
    }
}
